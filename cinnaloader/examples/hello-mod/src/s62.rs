//! Section 62 demo.

use cinnaloader::math::{
    self, Aabb, Easing, EulerAngles, Matrix4, Quaternion, Ray, Sphere, Vec2, Vec3,
};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(format!(
        "math(62): lerp(0,10,0.25)={} clamp(7,0,3)={} remap(5,0,10,100,200)={} smoothstep(0,10,5)={} ease(quad_in,0.5)={}",
        math::lerp(0.0, 10.0, 0.25),
        math::clamp(7.0, 0.0, 3.0),
        math::remap(5.0, 0.0, 10.0, 100.0, 200.0),
        math::smoothstep(0.0, 10.0, 5.0),
        math::ease(Easing::QuadIn, 0.5),
    ));

    let m = Matrix4::translation(1.0, 2.0, 3.0)
        * Matrix4::rotation_z(std::f64::consts::FRAC_PI_2)
        * Matrix4::scale(2.0, 2.0, 2.0);
    let inverse = m.inverse().expect("non-singular");
    lines.push(format!(
        "math(62): M times M inverse equals I within 1e-9: {} determinant(M)={} singular_refused={}",
        m.multiply(&inverse).approx_equal(&Matrix4::identity(), 1e-9),
        m.determinant(),
        Matrix4::scale(0.0, 1.0, 1.0).inverse().is_none(),
    ));
    lines.push(format!(
        "math(62): point vs direction transform: point.x={} direction.x={} (translation ignored)",
        Matrix4::translation(10.0, 0.0, 0.0).transform_point(Vec3::X).x,
        Matrix4::translation(10.0, 0.0, 0.0).transform_direction(Vec3::X).x,
    ));

    let euler = EulerAngles::new(90.0, 0.0, 0.0);
    let direction = euler.to_direction();
    let q = Quaternion::from_euler(euler);
    lines.push(format!(
        "math(62): euler yaw 90 to direction=({:.3},{:.3},{:.3}) quaternion rotate(Z) matches={}",
        direction.x,
        direction.y,
        direction.z,
        q.rotate(Vec3::Z).approx_eq(direction, 1e-9),
    ));
    lines.push(format!(
        "math(62): shortest path 350 to 10 at t=0.5 = {} (through 0) wrap_angle(190)={}",
        math::lerp_angle(350.0, 10.0, 0.5),
        math::wrap_angle(190.0),
    ));

    let noise = math::value_noise_2d(0.3, 0.7, 42);
    let near = math::value_noise_2d(0.3 + 1e-4, 0.7, 42);
    lines.push(format!(
        "math(62): value_noise(0.3,0.7,seed=42)={:.6} at +1e-4={:.6} delta={:.2e} continuous={}",
        noise,
        near,
        (noise - near).abs(),
        (noise - near).abs() < 1e-3,
    ));

    let mut rng = math::seeded_random(42);
    lines.push(format!(
        "math(62): splitmix64 seed=42 first three u64 = {},{},{}",
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64(),
    ));

    let sphere = Sphere::new(Vec3::ZERO, 1.0);
    let hit = math::intersect_ray(&Ray::new(Vec3::new(0.0, 0.0, -5.0), Vec3::Z), &sphere);
    let miss = math::intersect_ray(&Ray::new(Vec3::new(5.0, 0.0, -5.0), Vec3::Z), &sphere);
    let point = hit.map_or(Vec3::ZERO, |h| h.point);
    lines.push(format!(
        "math(62): ray-sphere hit point=({:.1},{:.1},{:.1}) distance={:.1} miss={}",
        point.x,
        point.y,
        point.z,
        hit.map_or(0.0, |h| h.distance),
        miss.is_none(),
    ));

    let a = Aabb::new(Vec3::ZERO, Vec3::splat(2.0));
    let b = Aabb::new(Vec3::ONE, Vec3::splat(3.0));
    let union = a.union(&b);
    lines.push(format!(
        "math(62): aabb union max=({:.0},{:.0},{:.0}) intersection size={:?} centre={:?}",
        union.max.x,
        union.max.y,
        union.max.z,
        a.intersection(&b).map(|x| x.size().x),
        a.center(),
    ));

    let cross2 = math::intersect_segments_2d(
        Vec2::new(0.0, 0.0),
        Vec2::new(2.0, 2.0),
        Vec2::new(0.0, 2.0),
        Vec2::new(2.0, 0.0),
    );
    lines.push(format!(
        "math(62): 2d segments cross at {:?} sphere-sphere contact={}",
        cross2,
        math::intersect_sphere(&Sphere::new(Vec3::ZERO, 1.0), &Sphere::new(Vec3::new(1.5, 0.0, 0.0), 1.0))
            .is_some(),
    ));

    lines
}
