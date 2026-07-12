use cgmath::{InnerSpace, Vector2, Vector3};

pub fn interpolate_uv(
    v: &[Vector3<f32>; 3],
    uv: &Option<[Vector2<f32>; 3]>,
    f: Vector3<f32>,
) -> Vector2<f32> {
    match uv {
        Some(uvs) => {
            let f0 = v[0] - f;
            let f1 = v[1] - f;
            let f2 = v[2] - f;

            let va = (v[0] - v[1]).cross(v[0] - v[2]);
            let va0 = f1.cross(f2);
            let va1 = f2.cross(f0);
            let va2 = f0.cross(f1);

            let a = va.magnitude();
            let a0 = va0.magnitude() / a * va.dot(va0).signum();
            let a1 = va1.magnitude() / a * va.dot(va1).signum();
            let a2 = va2.magnitude() / a * va.dot(va2).signum();

            uvs[0] * a0 + uvs[1] * a1 + uvs[2] * a2
        }
        None => Vector2::new(0., 0.),
    }
}
