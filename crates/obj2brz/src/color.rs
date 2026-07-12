use cgmath::Vector4;

pub fn ftoi(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.).round() as u8
}

pub fn itof(v: u8) -> f32 {
    (v as f32) / 255.
}

pub fn modulus(a: f32, b: f32) -> f32 {
    ((a % b) + b) % b
}

pub fn float_equals(a: f32, b: f32) -> bool {
    let error_margin = f32::EPSILON;
    (b - a).abs() < error_margin
}

pub fn rgb2hsv(rgb: Vector4<u8>) -> Vector4<f32> {
    let (r, g, b, a) = 
        (itof(rgb[0]), itof(rgb[1]), itof(rgb[2]), itof(rgb[3]));

    // max of rgb is equivalent to V in HSV
    let max = r.max(g).max(b);

    // min of rgb is V - C where C is chroma (range)
    let min = r.min(g).min(b);

    let mid = max - min;

    let mut hue = if float_equals(mid, 0.) {
        0.
    } else if float_equals(max, r) {
        modulus((g - b) / mid, 6.)
    } else if float_equals(max, g) {
        (b - r) / mid + 2.
    } else if float_equals(max, b) {
        (r - g) / mid + 4.
    } else {
        0.
    };

    hue *= std::f32::consts::PI / 3f32;
    if hue < 0. {
        hue += 2. * std::f32::consts::PI
    }

    let saturation = if float_equals(max, 0.) {
        0.
    } else {
        mid / max
    };

    Vector4::<f32>::new(hue, saturation, max, a)
}

fn color_conversion(color: u8) -> u8 {
    let color = itof(color);
    let c = if color > 0.04045 {
        ((color / 1.055) + 0.052_132_7).powf(2.4)
    } else {
        color / 12.92
    };
    ftoi(c)
}

pub fn gamma_correct(rgb: Vector4<u8>) -> Vector4<u8> {
    Vector4::<u8>::new(
        color_conversion(rgb[0]),
        color_conversion(rgb[1]),
        color_conversion(rgb[2]),
        rgb[3],
    )
}

pub fn hsv2rgb(hsv: Vector4<f32>) -> Vector4<u8> {
    let hue = hsv[0] * 180. / std::f32::consts::PI;
    let saturation = hsv[1];
    let value = hsv[2];

    let chroma = value * saturation;
    let x = chroma * (1f32 - (modulus(hue / 60., 2f32) - 1f32).abs());
    let match_value = value - chroma;

    let (r, g, b) = match hue {
        hh if hh < 60. => (chroma, x, 0.),
        hh if hh < 120. => (x, chroma, 0.),
        hh if hh < 180. => (0., chroma, x),
        hh if hh < 240. => (0., x, chroma),
        hh if hh < 300. => (x, 0., chroma),
        hh if hh < 360. => (chroma, 0., x),
        _ => (0., 0., 0.),
    };

    Vector4::new(
        ((r + match_value) * 255.) as u8,
        ((g + match_value) * 255.) as u8,
        ((b + match_value) * 255.) as u8,
        (hsv[3] * 255.) as u8,
    )
}

pub fn hsv_distance(a: &Vector4<f32>, b: &Vector4<f32>) -> f32 {
    (a.x.sin() * a.y - b.x.sin() * b.y).powf(2.0)
        + (a.x.cos() * a.y - b.x.cos() * b.y).powf(2.0)
        + (a.z - b.z).powf(2.0)
        + (a.w - b.w).powf(2.0)
}

pub fn hsv_average(colors: &[Vector4<u8>]) -> Vector4<f32> {
    // OBJ meshes can omit materials or UVs. Keep their fallback color stable
    // instead of producing NaNs from a zero-length average.
    if colors.is_empty() {
        return Vector4::new(0.0, 0.0, 1.0, 1.0);
    }

    let (mut h_sum, mut s_sum, mut v_sum, mut a_sum) = (0., 0., 0., 0.);
    for c in colors {
        let color = rgb2hsv(*c);
        h_sum += color.x;
        s_sum += color.y;
        v_sum += color.z;
        a_sum += color.w;
    }

    let n = colors.len() as f32;
    Vector4::<f32>::new(h_sum / n, s_sum / n, v_sum / n, a_sum / n)
}

pub fn convert_colorset_to_hsv(colorset: &[brdb::Color]) -> Vec<Vector4<f32>> {
    let mut converted_colorset = Vec::<Vector4<f32>>::with_capacity(colorset.len());
    for c in colorset {
        converted_colorset.push(rgb2hsv(Vector4::new(c.r, c.g, c.b, 255)));
    }

    converted_colorset
}

pub fn match_hsv_to_colorset(colorset: &[Vector4<f32>], color: &Vector4<f32>) -> usize {
    let mut min = 0;
    let mut min_distance = hsv_distance(&colorset[0], color);
    for (i, cs) in colorset.iter().enumerate() {
        let distance = hsv_distance(cs, color);
        if distance < min_distance {
            min_distance = distance;
            min = i;
        }
    }

    min
}
