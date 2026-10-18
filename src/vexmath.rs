// Matrix and quaternion maths for VEX, in Houdini's conventions:
// - matrices are row-major and vectors are rows, so a point transforms as v * M
//   (translation in the last row of a 4x4);
// - quaternions are {x, y, z, w};
// - Euler rotation orders follow VEX's XFORM_XYZ (0) ... XFORM_ZYX (5), where
//   XYZ rotates about X first, then Y, then Z.

pub type M3 = [f32; 9];
pub type M4 = [f32; 16];
pub type Q = [f32; 4];

pub fn m3_ident() -> M3 {
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn m4_ident() -> M4 {
    let mut m = [0.0; 16];
    for i in 0..4 {
        m[i * 5] = 1.0;
    }
    m
}

pub fn mul<const N: usize>(a: &[f32], b: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0; N * N];
    for i in 0..N {
        for j in 0..N {
            out[i * N + j] = (0..N).map(|k| a[i * N + k] * b[k * N + j]).sum();
        }
    }
    out
}

pub fn m3_mul(a: &M3, b: &M3) -> M3 {
    mul::<3>(a, b).try_into().unwrap()
}

pub fn m4_mul(a: &M4, b: &M4) -> M4 {
    mul::<4>(a, b).try_into().unwrap()
}

pub fn m3_to_m4(m: &M3) -> M4 {
    let mut out = m4_ident();
    for i in 0..3 {
        for j in 0..3 {
            out[i * 4 + j] = m[i * 3 + j];
        }
    }
    out
}

pub fn m4_to_m3(m: &M4) -> M3 {
    std::array::from_fn(|k| m[(k / 3) * 4 + k % 3])
}

pub fn transpose<const N: usize>(m: &[f32]) -> Vec<f32> {
    (0..N * N).map(|k| m[(k % N) * N + k / N]).collect()
}

// Determinant and inverse by Gauss-Jordan elimination (any size).
pub fn det(m: &[f32], n: usize) -> f32 {
    let mut a = m.to_vec();
    let mut d = 1.0;
    for c in 0..n {
        let p = (c..n).max_by(|&x, &y| a[x * n + c].abs().total_cmp(&a[y * n + c].abs())).unwrap();
        if a[p * n + c].abs() < 1e-20 {
            return 0.0;
        }
        if p != c {
            for k in 0..n {
                a.swap(p * n + k, c * n + k);
            }
            d = -d;
        }
        d *= a[c * n + c];
        for r in c + 1..n {
            let f = a[r * n + c] / a[c * n + c];
            for k in c..n {
                a[r * n + k] -= f * a[c * n + k];
            }
        }
    }
    d
}

pub fn invert(m: &[f32], n: usize) -> Option<Vec<f32>> {
    let mut a = m.to_vec();
    let mut inv: Vec<f32> = (0..n * n).map(|k| if k / n == k % n { 1.0 } else { 0.0 }).collect();
    for c in 0..n {
        let p = (c..n).max_by(|&x, &y| a[x * n + c].abs().total_cmp(&a[y * n + c].abs())).unwrap();
        if a[p * n + c].abs() < 1e-20 {
            return None;
        }
        for k in 0..n {
            a.swap(p * n + k, c * n + k);
            inv.swap(p * n + k, c * n + k);
        }
        let pivot = a[c * n + c];
        for k in 0..n {
            a[c * n + k] /= pivot;
            inv[c * n + k] /= pivot;
        }
        for r in 0..n {
            if r != c {
                let f = a[r * n + c];
                for k in 0..n {
                    a[r * n + k] -= f * a[c * n + k];
                    inv[r * n + k] -= f * inv[c * n + k];
                }
            }
        }
    }
    Some(inv)
}

// v * M for a 3x3 (rotation/scale) matrix.
pub fn v_m3(v: [f32; 3], m: &M3) -> [f32; 3] {
    std::array::from_fn(|j| (0..3).map(|i| v[i] * m[i * 3 + j]).sum())
}

// v * M for a 4x4 transform, treating v as a point (w = 1).
pub fn v_m4(v: [f32; 3], m: &M4) -> [f32; 3] {
    std::array::from_fn(|j| (0..3).map(|i| v[i] * m[i * 4 + j]).sum::<f32>() + m[12 + j])
}

// Rotation matrix (row-vector convention) about `axis` by `angle` radians.
pub fn rotation_m3(angle: f32, axis: [f32; 3]) -> M3 {
    q_to_m3(&q_axis_angle(angle, axis))
}

pub fn scale_m3(s: [f32; 3]) -> M3 {
    [s[0], 0.0, 0.0, 0.0, s[1], 0.0, 0.0, 0.0, s[2]]
}

pub fn translate_m4(t: [f32; 3]) -> M4 {
    let mut m = m4_ident();
    m[12..15].copy_from_slice(&t);
    m
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-20 { v.map(|c| c / l) } else { [0.0, 0.0, 0.0] }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn q_axis_angle(angle: f32, axis: [f32; 3]) -> Q {
    let a = normalize3(axis);
    let (s, c) = (angle * 0.5).sin_cos();
    [a[0] * s, a[1] * s, a[2] * s, c]
}

pub fn q_normalize(q: Q) -> Q {
    let l = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if l > 1e-20 { q.map(|c| c / l) } else { [0.0, 0.0, 0.0, 1.0] }
}

// Hamilton product a * b: rotating by the result applies b first, then a.
pub fn q_mul(a: &Q, b: &Q) -> Q {
    let (av, aw) = ([a[0], a[1], a[2]], a[3]);
    let (bv, bw) = ([b[0], b[1], b[2]], b[3]);
    let c = cross(av, bv);
    [aw * bv[0] + bw * av[0] + c[0], aw * bv[1] + bw * av[1] + c[1], aw * bv[2] + bw * av[2] + c[2], aw * bw - dot(av, bv)]
}

pub fn q_invert(q: &Q) -> Q {
    let n = q.iter().map(|c| c * c).sum::<f32>().max(1e-20);
    [-q[0] / n, -q[1] / n, -q[2] / n, q[3] / n]
}

pub fn q_rotate(q: &Q, v: [f32; 3]) -> [f32; 3] {
    let p = q_mul(&q_mul(q, &[v[0], v[1], v[2], 0.0]), &q_invert(q));
    [p[0], p[1], p[2]]
}

// The row-vector matrix that rotates like q: v * M == q_rotate(q, v).
pub fn q_to_m3(q: &Q) -> M3 {
    let [x, y, z, w] = q_normalize(*q);
    // Column (standard) rotation matrix, then transposed for row vectors.
    let r = [
        1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w),
        2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w),
        2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y),
    ];
    transpose::<3>(&r).try_into().unwrap()
}

pub fn q_from_m3(m: &M3) -> Q {
    // Back to the column matrix R = M^T.
    let r: [f32; 9] = transpose::<3>(m).try_into().unwrap();
    let at = |i: usize, j: usize| r[i * 3 + j];
    let trace = at(0, 0) + at(1, 1) + at(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(at(2, 1) - at(1, 2)) / s, (at(0, 2) - at(2, 0)) / s, (at(1, 0) - at(0, 1)) / s, 0.25 * s]
    } else if at(0, 0) > at(1, 1) && at(0, 0) > at(2, 2) {
        let s = (1.0 + at(0, 0) - at(1, 1) - at(2, 2)).sqrt() * 2.0;
        [0.25 * s, (at(0, 1) + at(1, 0)) / s, (at(0, 2) + at(2, 0)) / s, (at(2, 1) - at(1, 2)) / s]
    } else if at(1, 1) > at(2, 2) {
        let s = (1.0 + at(1, 1) - at(0, 0) - at(2, 2)).sqrt() * 2.0;
        [(at(0, 1) + at(1, 0)) / s, 0.25 * s, (at(1, 2) + at(2, 1)) / s, (at(0, 2) - at(2, 0)) / s]
    } else {
        let s = (1.0 + at(2, 2) - at(0, 0) - at(1, 1)).sqrt() * 2.0;
        [(at(0, 2) + at(2, 0)) / s, (at(1, 2) + at(2, 1)) / s, 0.25 * s, (at(1, 0) - at(0, 1)) / s]
    };
    q_normalize(q)
}

pub fn slerp(a: &Q, b: &Q, t: f32) -> Q {
    let mut b = *b;
    let mut cos = a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>();
    // Take the short way round.
    if cos < 0.0 {
        b = b.map(|c| -c);
        cos = -cos;
    }
    if cos > 0.9995 {
        return q_normalize(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
    }
    let theta = cos.clamp(-1.0, 1.0).acos();
    let (wa, wb) = (((1.0 - t) * theta).sin() / theta.sin(), (t * theta).sin() / theta.sin());
    std::array::from_fn(|i| a[i] * wa + b[i] * wb)
}

// The rotation taking direction a onto direction b.
pub fn dihedral(a: [f32; 3], b: [f32; 3]) -> Q {
    let (a, b) = (normalize3(a), normalize3(b));
    let d = dot(a, b);
    if d < -0.999999 {
        // Opposite: any perpendicular axis.
        let helper = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        return q_axis_angle(std::f32::consts::PI, cross(a, helper));
    }
    let c = cross(a, b);
    q_normalize([c[0], c[1], c[2], 1.0 + d])
}

// Axis order (first applied first) for VEX's XFORM_XYZ (0) ... XFORM_ZYX (5).
fn euler_axes(order: i32) -> [usize; 3] {
    match order {
        1 => [0, 2, 1],
        2 => [1, 0, 2],
        3 => [1, 2, 0],
        4 => [2, 0, 1],
        5 => [2, 1, 0],
        _ => [0, 1, 2],
    }
}

fn unit(axis: usize) -> [f32; 3] {
    let mut v = [0.0; 3];
    v[axis] = 1.0;
    v
}

pub fn euler_to_q(r: [f32; 3], order: i32) -> Q {
    let axes = euler_axes(order);
    // Apply the first axis first: q = q3 * q2 * q1.
    axes.iter().fold([0.0, 0.0, 0.0, 1.0], |acc, &a| q_mul(&q_axis_angle(r[a], unit(a)), &acc))
}

pub fn q_to_euler(q: &Q, order: i32) -> [f32; 3] {
    let [i, j, k] = euler_axes(order);
    // Column matrix R = Rk * Rj * Ri; `parity` is +1 for cyclic orders (XYZ, YZX, ZXY).
    let r: [f32; 9] = transpose::<3>(&q_to_m3(q)).try_into().unwrap();
    let at = |a: usize, b: usize| r[a * 3 + b];
    let parity = if (j + 3 - i) % 3 == 1 { 1.0 } else { -1.0 };
    let mut out = [0.0; 3];
    out[j] = (-parity * at(k, i)).clamp(-1.0, 1.0).asin();
    if at(k, i).abs() < 0.99999 {
        out[i] = (parity * at(k, j)).atan2(at(k, k));
        out[k] = (parity * at(j, i)).atan2(at(i, i));
    } else {
        // Gimbal lock: fold everything into the first axis.
        out[i] = (-parity * at(j, k)).atan2(at(j, j));
        out[k] = 0.0;
    }
    out
}

// Scale, then rotate (Euler degrees, XYZ by default), then translate.
pub fn make_transform(trs: i32, xyz: i32, t: [f32; 3], r_degrees: [f32; 3], s: [f32; 3]) -> M4 {
    let scale = m3_to_m4(&scale_m3(s));
    let rotate = m3_to_m4(&q_to_m3(&euler_to_q(r_degrees.map(f32::to_radians), xyz)));
    let translate = translate_m4(t);
    // VEX TRS orders: 0 SRT (the default), 1 STR, 2 RST, 3 RTS, 4 TSR, 5 TRS.
    let parts: [&M4; 3] = match trs {
        1 => [&scale, &translate, &rotate],
        2 => [&rotate, &scale, &translate],
        3 => [&rotate, &translate, &scale],
        4 => [&translate, &scale, &rotate],
        5 => [&translate, &rotate, &scale],
        _ => [&scale, &rotate, &translate],
    };
    m4_mul(&m4_mul(parts[0], parts[1]), parts[2])
}

// Rotation whose -Z axis points from `from` to `to`, with +Y toward `up`.
pub fn lookat(from: [f32; 3], to: [f32; 3], up: [f32; 3]) -> M3 {
    let forward = normalize3([to[0] - from[0], to[1] - from[1], to[2] - from[2]]);
    let z = forward.map(|c| -c);
    let mut x = normalize3(cross(up, z));
    if dot(x, x) < 1e-12 {
        x = normalize3(cross([1.0, 0.0, 0.0], z));
    }
    let y = cross(z, x);
    [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn quaternions_matrices_and_rotation_agree() {
        let q = q_axis_angle(0.9, [1.0, 2.0, 0.5]);
        let v = [0.3, -1.2, 2.0];
        assert!(close(&q_rotate(&q, v), &v_m3(v, &q_to_m3(&q))));
        assert!(close(&q_from_m3(&q_to_m3(&q)), &q) || close(&q_from_m3(&q_to_m3(&q)), &q.map(|c| -c)));
        // 90 degrees about Y takes +X to -Z.
        let r = q_axis_angle(std::f32::consts::FRAC_PI_2, [0.0, 1.0, 0.0]);
        assert!(close(&q_rotate(&r, [1.0, 0.0, 0.0]), &[0.0, 0.0, -1.0]));
        // q_mul applies the right-hand rotation first.
        let a = q_axis_angle(0.4, [0.0, 0.0, 1.0]);
        let both = q_mul(&a, &r);
        assert!(close(&q_rotate(&both, v), &q_rotate(&a, q_rotate(&r, v))));
        assert!(close(&q_rotate(&q_invert(&q), q_rotate(&q, v)), &v));
        let half = slerp(&[0.0, 0.0, 0.0, 1.0], &r, 0.5);
        assert!(close(&half, &q_axis_angle(std::f32::consts::FRAC_PI_4, [0.0, 1.0, 0.0])));
        let d = dihedral([1.0, 0.0, 0.0], [0.0, 1.0, 1.0]);
        let target = normalize3([0.0, 1.0, 1.0]);
        assert!(close(&q_rotate(&d, [1.0, 0.0, 0.0]), &target));
        assert!(close(&q_rotate(&dihedral([0.0, 0.0, 1.0], [0.0, 0.0, -1.0]), [0.0, 0.0, 1.0]), &[0.0, 0.0, -1.0]));
    }

    #[test]
    fn euler_round_trips_for_every_order() {
        let r = [0.3, -0.7, 1.1];
        for order in 0..6 {
            let q = euler_to_q(r, order);
            let back = q_to_euler(&q, order);
            assert!(close(&back, &r), "order {order}: {back:?}");
        }
        // XYZ: X is applied first.
        let q = euler_to_q([std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2, 0.0], 0);
        // +Y -> (X 90) -> +Z -> (Y 90) -> +X
        assert!(close(&q_rotate(&q, [0.0, 1.0, 0.0]), &[1.0, 0.0, 0.0]));
    }

    #[test]
    fn transforms_inverses_and_determinants() {
        let m = make_transform(0, 0, [1.0, 2.0, 3.0], [0.0, 90.0, 0.0], [2.0, 2.0, 2.0]);
        // Scale 2, rotate +X to -Z, then translate.
        assert!(close(&v_m4([1.0, 0.0, 0.0], &m), &[1.0, 2.0, 1.0]));
        let inv: M4 = invert(&m, 4).unwrap().try_into().unwrap();
        assert!(close(&m4_mul(&m, &inv), &m4_ident()));
        assert!((det(&m, 4) - 8.0).abs() < 1e-3);
        assert!(invert(&[1.0, 2.0, 2.0, 4.0], 2).is_none());
        assert!(close(&transpose::<3>(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]), &[1.0, 4.0, 7.0, 2.0, 5.0, 8.0, 3.0, 6.0, 9.0]));
        // lookat: -Z points at the target.
        let l = lookat([0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(close(&v_m3([0.0, 0.0, -1.0], &l), &[1.0, 0.0, 0.0]));
        assert!(close(&v_m3([0.0, 1.0, 0.0], &l), &[0.0, 1.0, 0.0]));
    }
}
