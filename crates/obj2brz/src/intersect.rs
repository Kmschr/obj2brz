use cgmath::{InnerSpace, Vector3};

fn find_min_max(f0: f32, f1: f32, f2: f32, min: &mut f32, max: &mut f32) {
    *min = f0;
    *max = f0;
    if *min > f1 {
        *min = f1
    };
    if *min > f2 {
        *min = f2
    };
    if *max < f1 {
        *max = f1
    };
    if *max < f2 {
        *max = f2
    };
}

/*======================== X-tests ========================*/

fn axis_test_x01(
    half_box: f32,
    v0: Vector3<f32>,
    v2: Vector3<f32>,
    a: f32,
    b: f32,
    fa: f32,
    fb: f32,
) -> bool {
    let p0 = a * v0[1] - b * v0[2];
    let p2 = a * v2[1] - b * v2[2];
    let min: f32;
    let max: f32;
    if p0 < p2 {
        min = p0;
        max = p2;
    } else {
        min = p2;
        max = p0;
    };
    let rad = (fa + fb) * half_box;
    !(min > rad || max < -rad)
}

fn axis_test_x2(
    half_box: f32,
    v0: Vector3<f32>,
    v1: Vector3<f32>,
    a: f32,
    b: f32,
    fa: f32,
    fb: f32,
) -> bool {
    let p0 = a * v0[1] - b * v0[2];
    let p1 = a * v1[1] - b * v1[2];
    let min: f32;
    let max: f32;
    if p0 < p1 {
        min = p0;
        max = p1;
    } else {
        min = p1;
        max = p0;
    };
    let rad = (fa + fb) * half_box;
    !(min > rad || max < -rad)
}

/*======================== Y-tests ========================*/

fn axis_test_y02(
    half_box: f32,
    v0: Vector3<f32>,
    v2: Vector3<f32>,
    a: f32,
    b: f32,
    fa: f32,
    fb: f32,
) -> bool {
    let p0 = -a * v0[0] + b * v0[2];
    let p2 = -a * v2[0] + b * v2[2];
    let min: f32;
    let max: f32;
    if p0 < p2 {
        min = p0;
        max = p2;
    } else {
        min = p2;
        max = p0;
    };
    let rad = (fa + fb) * half_box;
    !(min > rad || max < -rad)
}

fn axis_test_y1(
    half_box: f32,
    v0: Vector3<f32>,
    v1: Vector3<f32>,
    a: f32,
    b: f32,
    fa: f32,
    fb: f32,
) -> bool {
    let p0 = -a * v0[0] + b * v0[2];
    let p1 = -a * v1[0] + b * v1[2];
    let min: f32;
    let max: f32;
    if p0 < p1 {
        min = p0;
        max = p1;
    } else {
        min = p1;
        max = p0;
    };
    let rad = (fa + fb) * half_box;
    !(min > rad || max < -rad)
}

/*======================== Z-tests ========================*/

fn axis_test_z12(
    half_box: f32,
    v1: Vector3<f32>,
    v2: Vector3<f32>,
    a: f32,
    b: f32,
    fa: f32,
    fb: f32,
) -> bool {
    let p1 = a * v1[0] - b * v1[1];
    let p2 = a * v2[0] - b * v2[1];
    let min: f32;
    let max: f32;
    if p2 < p1 {
        min = p2;
        max = p1;
    } else {
        min = p1;
        max = p2;
    };
    let rad = (fa + fb) * half_box;
    !(min > rad || max < -rad)
}

fn axis_test_z0(
    half_box: f32,
    v0: Vector3<f32>,
    v1: Vector3<f32>,
    a: f32,
    b: f32,
    fa: f32,
    fb: f32,
) -> bool {
    let p0 = a * v0[0] - b * v0[1];
    let p1 = a * v1[0] - b * v1[1];
    let min: f32;
    let max: f32;
    if p0 < p1 {
        min = p0;
        max = p1;
    } else {
        min = p1;
        max = p0;
    };
    let rad = (fa + fb) * half_box;
    !(min > rad || max < -rad)
}

fn plane_box_overlap(half_box: f32, normal: Vector3<f32>, vert: Vector3<f32>) -> bool {
    let mut vmin = Vector3::<f32>::new(0.0, 0.0, 0.0);
    let mut vmax = Vector3::<f32>::new(0.0, 0.0, 0.0);

    for q in 0..3 {
        let v = vert[q];
        if normal[q] > 0.0 {
            vmin[q] = -half_box - v;
            vmax[q] = half_box - v;
        } else {
            vmin[q] = half_box - v;
            vmax[q] = -half_box - v;
        }
    }

    if normal.dot(vmin) > 0.0 {
        return false;
    };
    if normal.dot(vmax) >= 0.0 {
        return true;
    };

    false
}

pub fn intersect(
    half_box: f32,
    center: Vector3<f32>,
    p0: Vector3<f32>,
    p1: Vector3<f32>,
    p2: Vector3<f32>,
) -> Option<Vector3<f32>> {
    let v0 = p0 - center;
    let v1 = p1 - center;
    let v2 = p2 - center;

    let e0 = v1 - v0;
    let e1 = v2 - v1;
    let e2 = v0 - v2;

    let mut fe = Vector3::<f32>::new(e0[0].abs(), e0[1].abs(), e0[2].abs());
    if !axis_test_x01(half_box, v0, v2, e0[2], e0[1], fe[2], fe[1]) {
        return None;
    };
    if !axis_test_y02(half_box, v0, v2, e0[2], e0[0], fe[2], fe[0]) {
        return None;
    };
    if !axis_test_z12(half_box, v1, v2, e0[1], e0[0], fe[1], fe[0]) {
        return None;
    };

    fe = Vector3::<f32>::new(e1[0].abs(), e1[1].abs(), e1[2].abs());
    if !axis_test_x01(half_box, v0, v2, e1[2], e1[1], fe[2], fe[1]) {
        return None;
    };
    if !axis_test_y02(half_box, v0, v2, e1[2], e1[0], fe[2], fe[0]) {
        return None;
    };
    if !axis_test_z0(half_box, v0, v1, e1[1], e1[0], fe[1], fe[0]) {
        return None;
    };

    fe = Vector3::<f32>::new(e2[0].abs(), e2[1].abs(), e2[2].abs());
    if !axis_test_x2(half_box, v0, v1, e2[2], e2[1], fe[2], fe[1]) {
        return None;
    };
    if !axis_test_y1(half_box, v0, v1, e2[2], e2[0], fe[2], fe[0]) {
        return None;
    };
    if !axis_test_z12(half_box, v1, v2, e2[1], e2[0], fe[1], fe[0]) {
        return None;
    };

    let mut min: f32 = 0.0;
    let mut max: f32 = 0.0;
    find_min_max(v0[0], v1[0], v2[0], &mut min, &mut max);
    if min > half_box || max < -half_box {
        return None;
    };

    find_min_max(v0[1], v1[1], v2[1], &mut min, &mut max);
    if min > half_box || max < -half_box {
        return None;
    };

    find_min_max(v0[2], v1[2], v2[2], &mut min, &mut max);
    if min > half_box || max < -half_box {
        return None;
    };

    let normal = e0.cross(e1);
    if !plane_box_overlap(half_box, normal, v0) {
        return None;
    };

    // Orthogonal projection to determine UV coordinates
    Some((v0.dot(normal) * normal) / normal.dot(normal) + center)
}
