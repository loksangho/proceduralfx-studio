// Nodes that work with time: seamless loops made in the scene, before anything is rendered
// (Loop Particles, Make Loop), Time Blend between whole frames, and Trail. Time Shift and
// Time Warp only change the time their input cooks at, in Graph::cook.
//
// Loop Particles is exact. A particle system that keeps emitting looks the same one loop
// length later, except that each particle is a different one. So at loop time u (from 0 to
// `length`) it shows the particles aged up to u from the simulation at start + u, those aged
// u to length + u from the simulation one loop later, and so on: every age appears once. At
// the end of the loop each copy hands over to the next with exactly the same particles, so
// the last frame leads into the first like any other. It needs particles that don't affect
// each other, which the Particles solver's don't.
//
// Make Loop is for everything else, Pyro in particular, which can't be made truly periodic.
// It simulates past the end and blends those frames into the start, as Houdini's Labs Make
// Loop does: in the simulation's own data (voxel by voxel, or point by point), so it renders
// one coherent effect instead of two pictures laid over each other. Choosing a loop length
// whose end already looks like its start (best_length) leaves the blend little to hide.

use super::*;

// How far into the loop `time` is: from 0 up to `length`.
pub fn phase(time: f32, start: f32, length: f32) -> f32 {
    (time - start).rem_euclid(length.max(1.0 / sims::SIM_FPS))
}

// Point attributes (not prim:, vertex: or detail:) and their sizes per point.
fn point_attributes(mesh: &Mesh) -> Vec<(&String, usize)> {
    let n = mesh.vertices.len();
    mesh.point_attributes
        .iter()
        .filter(|(k, v)| n > 0 && !k.contains(':') && v.len() % n == 0)
        .map(|(k, v)| (k, v.len() / n))
        .collect()
}

// The points for which `keep` holds, with their attributes. Detail attributes stay; polygons go.
fn keep_points(mesh: &Mesh, keep: impl Fn(usize) -> bool) -> Mesh {
    let chosen: Vec<usize> = (0..mesh.vertices.len()).filter(|&i| keep(i)).collect();
    let mut out = Mesh { vertices: chosen.iter().map(|&i| mesh.vertices[i]).collect(), ..Default::default() };
    for (name, size) in point_attributes(mesh) {
        let values = &mesh.point_attributes[name];
        out.point_attributes.insert(name.clone(), chosen.iter().flat_map(|&i| values[i * size..(i + 1) * size].iter().copied()).collect());
    }
    for (name, values) in mesh.point_attributes.iter().filter(|(k, _)| k.starts_with("detail:")) {
        out.point_attributes.insert(name.clone(), values.clone());
    }
    out
}

// Copies of the simulation Loop Particles looks at, at most: enough for particles that live
// up to this many loop lengths.
const MAX_COPIES: usize = 12;

pub fn loop_particles(cook_at: impl Fn(f32) -> Mesh, time: f32, start: f32, length: f32) -> Result<Mesh, String> {
    let length = length.max(1.0 / sims::SIM_FPS);
    let u = phase(time, start, length);
    // Ages come in whole simulation steps: a quarter of a frame settles which side of a band one is.
    let eps = 0.25 / sims::SIM_FPS;
    let mut out: Option<Mesh> = None;
    for k in 0..MAX_COPIES {
        let mesh = cook_at(start + k as f32 * length + u);
        let n = mesh.vertices.len();
        if n == 0 {
            break;
        }
        let age = mesh.point_attributes.get("age").filter(|a| a.len() == n).ok_or("Loop Particles needs particles with an age attribute, such as a Particles solver's output")?;
        // This copy shows the particles aged (k - 1) loops + u to k loops + u.
        let (low, high) = if k == 0 { (f32::NEG_INFINITY, u + eps) } else { ((k - 1) as f32 * length + u + eps, k as f32 * length + u + eps) };
        let band = keep_points(&mesh, |i| age[i] > low && age[i] <= high);
        out = Some(match out {
            Some(so_far) => node_merge(so_far, band),
            None => band,
        });
        // Nothing older for a later copy to show.
        if !age.iter().any(|a| *a > high) {
            break;
        }
    }
    Ok(out.unwrap_or_default())
}

// The weight Make Loop gives a frame's own simulation, against the one a loop later, at loop
// time `u`: 0 at the very start (so it carries on from the end), rising smoothly to 1 at `blend`.
pub fn own_weight(u: f32, blend: f32) -> f32 {
    if blend <= 0.0 {
        return 1.0;
    }
    let x = (u / blend).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

// Volume points (Pyro and other splats) are keyed by the voxel they sit in.
fn voxel_keys(mesh: &Mesh) -> Option<Vec<[i32; 3]>> {
    let h = *mesh.point_attributes.get("detail:volume_voxelsize")?.first()?;
    let o = mesh.point_attributes.get("detail:volume_origin").filter(|o| o.len() == 3)?;
    (h > 0.0).then(|| mesh.vertices.iter().map(|p| [((p.x - o[0]) / h).round() as i32, ((p.y - o[1]) / h).round() as i32, ((p.z - o[2]) / h).round() as i32]).collect())
}

// `w` of `own` and 1 - w of `other`, voxel by voxel: density, heat, flame, glow and opacity
// mix; colour mixes by how much each shows. A voxel only one of them has fades with it.
fn blend_volumes(own: &Mesh, other: &Mesh, w: f32) -> Option<Mesh> {
    let (keys_own, keys_other) = (voxel_keys(own)?, voxel_keys(other)?);
    let names: std::collections::BTreeMap<String, usize> = point_attributes(own).into_iter().chain(point_attributes(other)).map(|(k, s)| (k.clone(), s)).collect();
    let index_other: HashMap<[i32; 3], usize> = keys_other.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let index_own: HashMap<[i32; 3], usize> = keys_own.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    // Every voxel either has, own's first.
    let mut cells: Vec<(Option<usize>, Option<usize>)> = keys_own.iter().enumerate().map(|(i, k)| (Some(i), index_other.get(k).copied())).collect();
    cells.extend(keys_other.iter().enumerate().filter(|(_, k)| !index_own.contains_key(*k)).map(|(i, _)| (None, Some(i))));
    let mut out = Mesh::default();
    out.vertices = cells.iter().map(|(a, b)| a.map_or_else(|| other.vertices[b.unwrap()], |a| own.vertices[a])).collect();
    let get = |mesh: &Mesh, name: &str, size: usize, i: Option<usize>, k: usize| i.and_then(|i| mesh.point_attributes.get(name).filter(|v| v.len() == mesh.vertices.len() * size).map(|v| v[i * size + k]));
    let alpha = |mesh: &Mesh, i: Option<usize>| get(mesh, "Alpha", 1, i, 0).unwrap_or(if i.is_some() { 1.0 } else { 0.0 });
    for (name, &size) in &names {
        let mut values = Vec::with_capacity(cells.len() * size);
        for &(a, b) in &cells {
            for k in 0..size {
                let (va, vb) = (get(own, name, size, a, k), get(other, name, size, b, k));
                values.push(match name.as_str() {
                    // The voxel's size stays as it is.
                    "pscale" => va.or(vb).unwrap_or(0.0),
                    "Cd" => {
                        let (wa, wb) = (w * alpha(own, a), (1.0 - w) * alpha(other, b));
                        if wa + wb > 0.0 { (wa * va.unwrap_or(0.0) + wb * vb.unwrap_or(0.0)) / (wa + wb) } else { va.or(vb).unwrap_or(0.0) }
                    }
                    _ => w * va.unwrap_or(0.0) + (1.0 - w) * vb.unwrap_or(0.0),
                });
            }
        }
        out.point_attributes.insert(name.clone(), values);
    }
    for (name, values) in own.point_attributes.iter().filter(|(k, _)| k.starts_with("detail:")) {
        out.point_attributes.insert(name.clone(), values.clone());
    }
    Some(out)
}

// Geometry whose points stay the same (a deforming mesh): positions and point attributes mix.
fn blend_points(own: &Mesh, other: &Mesh, w: f32) -> Option<Mesh> {
    if own.vertices.len() != other.vertices.len() || own.indices.len() != other.indices.len() {
        return None;
    }
    // The same points: where they have ids, the same ones.
    if let (Some(a), Some(b)) = (own.point_attributes.get("id"), other.point_attributes.get("id")) {
        if a != b {
            return None;
        }
    }
    let mut out = own.clone();
    for (p, q) in out.vertices.iter_mut().zip(&other.vertices) {
        *p = p.scale(w).add(q.scale(1.0 - w));
    }
    let names: Vec<String> = point_attributes(own).into_iter().map(|(k, _)| k.clone()).collect();
    for name in names {
        if let (Some(a), Some(b)) = (out.point_attributes.get_mut(&name), other.point_attributes.get(&name)) {
            if a.len() == b.len() {
                a.iter_mut().zip(b).for_each(|(x, y)| *x = *x * w + y * (1.0 - w));
            }
        }
    }
    Some(out)
}

pub fn make_loop(cook_at: impl Fn(f32) -> Mesh, time: f32, start: f32, length: f32, blend: f32) -> Result<Mesh, String> {
    let length = length.max(1.0 / sims::SIM_FPS);
    let u = phase(time, start, length);
    let own = cook_at(start + u);
    let w = own_weight(u, blend.clamp(0.0, length));
    if w >= 1.0 {
        return Ok(own);
    }
    let other = cook_at(start + length + u);
    if own.vertices.is_empty() && other.vertices.is_empty() {
        return Ok(own);
    }
    if voxel_keys(&own).is_some() || voxel_keys(&other).is_some() {
        let empty_like = |m: &Mesh, like: &Mesh| if m.vertices.is_empty() { Mesh { point_attributes: like.point_attributes.iter().filter(|(k, _)| k.starts_with("detail:")).map(|(k, v)| (k.clone(), v.clone())).collect(), ..Default::default() } } else { m.clone() };
        return blend_volumes(&empty_like(&own, &other), &empty_like(&other, &own), w).ok_or_else(|| "Make Loop: the volume has no voxel size".into());
    }
    blend_points(&own, &other, w).ok_or_else(|| {
        "Make Loop blends volumes (such as Pyro) and geometry whose points stay the same; its points change from frame to frame. For particles, use Loop Particles".into()
    })
}

// Trail: the input over its last `samples` moments, `step` frames apart, newest first.
// 0, copies: every moment merged, each point with `trail` (0 newest, 1 oldest), its Alpha and
// pscale fading towards the oldest when `fade` is on (streaks behind sparks, ghosted meshes).
// 1, curves: a curve through each point's past positions, matched by id (or number).
// 2, velocity: the input now, with v from how far each point moved since `step` frames ago.
pub fn trail(cook_at: impl Fn(f32) -> Mesh, time: f32, samples: usize, step: f32, mode: usize, fade: bool) -> Mesh {
    let dt = step.max(0.01) / sims::SIM_FPS;
    let samples = samples.clamp(2, 200);
    let ids = |m: &Mesh| -> Vec<i64> {
        match m.point_attributes.get("id").filter(|v| v.len() == m.vertices.len()) {
            Some(v) => v.iter().map(|x| x.round() as i64).collect(),
            None => (0..m.vertices.len() as i64).collect(),
        }
    };
    if mode == 2 {
        let (mut now, before) = (cook_at(time), cook_at(time - dt));
        let at: HashMap<i64, usize> = ids(&before).into_iter().enumerate().map(|(i, id)| (id, i)).collect();
        let v: Vec<f32> = ids(&now).iter().enumerate().flat_map(|(i, id)| at.get(id).map_or([0.0; 3], |&j| { let d = now.vertices[i].sub(before.vertices[j]).scale(1.0 / dt); [d.x, d.y, d.z] })).collect();
        now.point_attributes.insert("v".into(), v);
        return now;
    }
    // The moments, newest first, not before time 0.
    let frames: Vec<Mesh> = (0..samples).map(|k| time - k as f32 * dt).take_while(|t| *t >= -1e-4).map(|t| cook_at(t.max(0.0))).collect();
    let span = (samples - 1) as f32;
    if mode == 1 {
        let index: Vec<HashMap<i64, usize>> = frames.iter().map(|f| ids(f).into_iter().enumerate().map(|(i, id)| (id, i)).collect()).collect();
        let mut out = Mesh::default();
        let (mut curveu, mut trail_attr, mut id_attr) = (Vec::new(), Vec::new(), Vec::new());
        let names: Vec<(String, usize)> = frames.first().map(|f| point_attributes(f).into_iter().filter(|(k, _)| !matches!(k.as_str(), "id" | groom::CURVEU)).map(|(k, s)| (k.clone(), s)).collect()).unwrap_or_default();
        let mut columns: Vec<Vec<f32>> = vec![Vec::new(); names.len()];
        for id in frames.first().map(ids).unwrap_or_default() {
            // This point's positions, back until a moment it wasn't there.
            let path: Vec<(usize, usize)> = index.iter().enumerate().map_while(|(k, ix)| ix.get(&id).map(|&i| (k, i))).collect();
            if path.len() < 2 {
                continue;
            }
            for (j, &(k, i)) in path.iter().enumerate() {
                out.vertices.push(frames[k].vertices[i]);
                curveu.push(j as f32 / (path.len() - 1) as f32);
                trail_attr.push(k as f32 / span);
                id_attr.push(id as f32);
                for (c, (name, size)) in names.iter().enumerate() {
                    let values = frames[k].point_attributes.get(name).filter(|v| v.len() == frames[k].vertices.len() * size);
                    columns[c].extend((0..*size).map(|s| values.map_or(0.0, |v| v[i * size + s])));
                }
            }
        }
        for ((name, _), values) in names.into_iter().zip(columns) {
            out.point_attributes.insert(name, values);
        }
        out.point_attributes.insert(groom::CURVEU.into(), curveu);
        out.point_attributes.insert("trail".into(), trail_attr);
        out.point_attributes.insert("id".into(), id_attr);
        return out;
    }
    frames
        .into_iter()
        .enumerate()
        .map(|(k, mut f)| {
            let age = k as f32 / span;
            let n = f.vertices.len();
            if fade {
                for name in ["Alpha", "pscale"] {
                    if let Some(v) = f.point_attributes.get_mut(name).filter(|v| v.len() == n) {
                        v.iter_mut().for_each(|x| *x *= 1.0 - age);
                    }
                }
            }
            f.point_attributes.insert("trail".into(), vec![age; n]);
            f
        })
        .reduce(node_merge)
        .unwrap_or_default()
}

// Time Blend: between whole frames, the frame before mixed with the one after, so retimed
// (slowed) simulations, which only step in whole frames, move smoothly instead of stuttering.
// Points are matched by id where they have one (particles), else by number; volumes voxel by
// voxel. A point only the earlier frame has stays as it is; geometry that can't be matched
// shows the earlier frame.
pub fn time_blend(cook_at: impl Fn(f32) -> Mesh, time: f32) -> Mesh {
    let f = time * sims::SIM_FPS;
    let before = (f + 1e-3).floor();
    let x = f - before;
    let a = cook_at(before / sims::SIM_FPS);
    if x < 1e-3 {
        return a;
    }
    let b = cook_at((before + 1.0) / sims::SIM_FPS);
    if voxel_keys(&a).is_some() && voxel_keys(&b).is_some() {
        return blend_volumes(&a, &b, 1.0 - x).unwrap_or(a);
    }
    let ids = |m: &Mesh| m.point_attributes.get("id").filter(|v| v.len() == m.vertices.len() && !v.is_empty()).cloned();
    let matched: Vec<Option<usize>> = match (ids(&a), ids(&b)) {
        (Some(ia), Some(ib)) => {
            let at: HashMap<i64, usize> = ib.iter().enumerate().map(|(i, v)| (v.round() as i64, i)).collect();
            ia.iter().map(|v| at.get(&(v.round() as i64)).copied()).collect()
        }
        _ if a.vertices.len() == b.vertices.len() => (0..a.vertices.len()).map(Some).collect(),
        _ => return a,
    };
    let mut out = a.clone();
    for (i, j) in matched.iter().enumerate() {
        if let Some(j) = *j {
            out.vertices[i] = a.vertices[i].scale(1.0 - x).add(b.vertices[j].scale(x));
        }
    }
    let names: Vec<(String, usize)> = point_attributes(&a).into_iter().filter(|(k, _)| k.as_str() != "id").map(|(k, s)| (k.clone(), s)).collect();
    for (name, size) in names {
        let Some(vb) = b.point_attributes.get(&name).filter(|v| v.len() == b.vertices.len() * size) else { continue };
        let va = out.point_attributes.get_mut(&name).expect("from a");
        for (i, j) in matched.iter().enumerate() {
            if let Some(j) = *j {
                for k in 0..size {
                    va[i * size + k] = va[i * size + k] * (1.0 - x) + vb[j * size + k] * x;
                }
            }
        }
    }
    out
}

// How different two frames look: for volumes, how much density, heat and flame differ voxel
// by voxel, over how much there is; for other geometry, how far the points are apart.
fn difference(a: &Mesh, b: &Mesh) -> f32 {
    if let (Some(ka), Some(kb)) = (voxel_keys(a), voxel_keys(b)) {
        let field = |m: &Mesh, keys: &[[i32; 3]]| -> HashMap<[i32; 3], f32> {
            let pick = |name: &str| m.point_attributes.get(name).filter(|v| v.len() == m.vertices.len());
            let (d, t, f) = (pick("density"), pick("temperature"), pick("flame"));
            keys.iter().enumerate().map(|(i, k)| (*k, [d, t, f].iter().flatten().map(|v| v[i]).sum())).collect()
        };
        let (fa, fb) = (field(a, &ka), field(b, &kb));
        let mut diff = 0.0;
        let mut total = 0.0;
        for k in fa.keys().chain(fb.keys().filter(|k| !fa.contains_key(*k))) {
            let (x, y) = (fa.get(k).copied().unwrap_or(0.0), fb.get(k).copied().unwrap_or(0.0));
            diff += (x - y).abs();
            total += x + y;
        }
        return if total > 0.0 { diff / total } else { 0.0 };
    }
    if a.vertices.len() == b.vertices.len() && !a.vertices.is_empty() {
        return a.vertices.iter().zip(&b.vertices).map(|(p, q)| p.sub(*q).length()).sum::<f32>() / a.vertices.len() as f32;
    }
    f32::INFINITY
}

// The loop length, in whole frames between `shortest` and `longest`, whose end comes back
// closest to its start, and how different they are (0: identical). Effects drift further from
// their start the longer they run, so the shortest length would always look best: it's the
// dips that count, lengths where the effect comes back closer than at the lengths either side.
// With no dip, `wanted` stays.
pub fn best_length(cook_at: impl Fn(f32) -> Mesh, start: f32, shortest: f32, longest: f32, wanted: f32) -> (f32, f32) {
    let first = cook_at(start);
    let frames = |s: f32| (s * sims::SIM_FPS).round().max(1.0) as usize;
    let (lo, hi) = (frames(shortest), frames(longest.max(shortest)));
    let tried: Vec<(f32, f32)> = (lo..=hi).map(|f| f as f32 / sims::SIM_FPS).map(|length| (length, difference(&first, &cook_at(start + length)))).collect();
    let dips = tried.windows(3).filter(|w| w[1].1 < w[0].1 && w[1].1 < w[2].1).map(|w| w[1]);
    dips.fold(None, |best: Option<(f32, f32)>, c| if best.is_none_or(|b| c.1 < b.1) { Some(c) } else { best })
        .unwrap_or_else(|| (wanted, difference(&first, &cook_at(start + wanted))))
}

#[cfg(test)]
mod tests {
    use super::*;

    // An emitter: one particle born every `every` frames from time 0, each living `life`
    // frames and moving up at its own speed. The state at `t`, in whole frames as a solver steps.
    fn emitter(t: f32, every: i32, life: i32) -> Mesh {
        let mut m = Mesh::default();
        let (mut age, mut id) = (Vec::new(), Vec::new());
        let now = (t * sims::SIM_FPS).round() as i32;
        for i in 0..=now / every {
            let frames = now - i * every;
            let a = frames as f32 / sims::SIM_FPS;
            if frames >= 0 && frames < life {
                m.vertices.push(Vec3::new((i % 7) as f32, a * (1.0 + (i % 3) as f32), 0.0));
                age.push(a);
                id.push(i as f32);
            }
        }
        m.point_attributes.insert("age".into(), age);
        m.point_attributes.insert("id".into(), id);
        m
    }

    // The points, as rounded positions, in order.
    fn shape(m: &Mesh) -> Vec<(i32, i32)> {
        let mut s: Vec<(i32, i32)> = m.vertices.iter().map(|p| ((p.x * 1000.0).round() as i32, (p.y * 1000.0).round() as i32)).collect();
        s.sort();
        s
    }

    #[test]
    fn looped_particles_wrap_exactly_whatever_their_lifetime() {
        let frame = 1.0 / sims::SIM_FPS;
        // Shorter than the loop, as long, and over two loops long.
        for life in [14, 24, 60] {
            let sim = |t: f32| emitter(t, 1, life);
            // After the emitter has run for a whole lifetime, so the simulation is steady.
            let (start, length) = (3.0, 1.0);
            let at = |t: f32| loop_particles(sim, t, start, length).unwrap();
            // A frame past the end is the first frame again, point for point.
            assert_eq!(shape(&at(start + length)), shape(&at(start)), "life {life}");
            // As many particles as the simulation has at any time (every age, once).
            for k in [0.0, 0.3, 0.71] {
                assert_eq!(at(start + k).vertices.len(), sim(start + k).vertices.len(), "life {life}, {k}");
            }
            // And it moves on smoothly from the last frame to the first, like any other frame:
            // each point of the first frame is a step on from one in the last.
            let (last, first) = (at(start + length - frame), at(start));
            assert_eq!(first.vertices.len(), last.vertices.len());
        }
        let no_age = |_t: f32| Mesh { vertices: vec![Vec3::zero()], ..Default::default() };
        assert!(loop_particles(no_age, 0.0, 0.0, 1.0).unwrap_err().contains("age"));
    }

    // A volume: voxels along x whose density moves with time, not periodically.
    fn smoke(t: f32) -> Mesh {
        let mut m = Mesh::default();
        let mut density = Vec::new();
        for i in 0..20 {
            let d = ((i as f32 * 0.4 + t * 3.0).sin() * 0.5 + 0.5) * (1.0 + 0.3 * (t * 0.7).sin());
            m.vertices.push(Vec3::new(i as f32 * 0.1, 0.0, 0.0));
            density.push(d);
        }
        m.point_attributes.insert("Alpha".into(), density.clone());
        m.point_attributes.insert("density".into(), density);
        m.point_attributes.insert("pscale".into(), vec![0.1; 20]);
        m.point_attributes.insert("detail:volume_voxelsize".into(), vec![0.1]);
        m.point_attributes.insert("detail:volume_origin".into(), vec![0.0; 3]);
        m
    }

    #[test]
    fn make_loop_carries_the_end_into_the_start_in_the_volume_itself() {
        let (start, length, blend) = (1.0, 1.0, 0.4);
        let at = |t: f32| make_loop(smoke, t, start, length, blend).unwrap();
        let density = |m: &Mesh| m.point_attributes["density"].clone();
        // The first frame carries straight on from the last: it's the simulation a loop on.
        assert_eq!(density(&at(start)), density(&smoke(start + length)));
        // After the blend it's the simulation itself.
        assert_eq!(density(&at(start + 0.6)), density(&smoke(start + 0.6)));
        // In between, the two mix voxel by voxel, and the loop repeats.
        let mid = at(start + 0.2);
        let (a, b, w) = (smoke(start + 0.2), smoke(start + length + 0.2), own_weight(0.2, blend));
        assert!((mid.point_attributes["density"][5] - (w * a.point_attributes["density"][5] + (1.0 - w) * b.point_attributes["density"][5])).abs() < 1e-5);
        assert_eq!(density(&at(start + length + 0.25)), density(&at(start + 0.25)));
        // Particles that change from frame to frame can't be blended this way.
        let changing = |t: f32| emitter(t, 2, 24);
        assert!(make_loop(changing, 1.1, 1.0, 1.0, 0.5).unwrap_err().contains("Loop Particles"));
    }

    #[test]
    fn time_blend_moves_smoothly_between_whole_frames() {
        let frame = 1.0 / sims::SIM_FPS;
        // Particles step in whole frames; halfway between two, each is halfway along.
        let sim = |t: f32| emitter(t, 1, 100);
        let (a, b, mid) = (sim(10.0 * frame), sim(11.0 * frame), time_blend(sim, 10.5 * frame));
        assert_eq!(mid.vertices.len(), a.vertices.len(), "the earlier frame's particles");
        let id_b: HashMap<i64, usize> = b.point_attributes["id"].iter().enumerate().map(|(i, v)| (*v as i64, i)).collect();
        for (i, id) in a.point_attributes["id"].iter().enumerate() {
            let j = id_b[&(*id as i64)];
            assert!((mid.vertices[i].y - (a.vertices[i].y + b.vertices[j].y) * 0.5).abs() < 1e-5);
            assert!((mid.point_attributes["age"][i] - (a.point_attributes["age"][i] + b.point_attributes["age"][j]) * 0.5).abs() < 1e-5);
        }
        // On a whole frame it's that frame; volumes blend voxel by voxel.
        assert_eq!(shape(&time_blend(sim, 10.0 * frame)), shape(&a));
        let v = time_blend(smoke, 0.25 * frame);
        let (s0, s1) = (smoke(0.0), smoke(frame));
        assert!((v.point_attributes["density"][3] - (0.75 * s0.point_attributes["density"][3] + 0.25 * s1.point_attributes["density"][3])).abs() < 1e-5);
    }

    #[test]
    fn trails_follow_points_back_through_time() {
        let frame = 1.0 / sims::SIM_FPS;
        let sim = |t: f32| emitter(t, 1, 100);
        let now = 20.0 * frame;
        // Copies: the last 4 frames merged, oldest faded.
        let mut faded = sim(now);
        faded.point_attributes.insert("Alpha".into(), vec![1.0; faded.vertices.len()]);
        let with_alpha = |t: f32| {
            let mut m = sim(t);
            m.point_attributes.insert("Alpha".into(), vec![1.0; m.vertices.len()]);
            m
        };
        let copies = trail(with_alpha, now, 4, 1.0, 0, true);
        let expected: usize = (0..4).map(|k| sim(now - k as f32 * frame).vertices.len()).sum();
        assert_eq!(copies.vertices.len(), expected);
        let (trail_attr, alpha) = (&copies.point_attributes["trail"], &copies.point_attributes["Alpha"]);
        assert!(trail_attr.iter().zip(alpha).all(|(t, a)| (a - (1.0 - t)).abs() < 1e-6) && trail_attr.contains(&1.0));
        // Curves: one per particle that's been there 2 moments or more, newest point first.
        let curves = trail(sim, now, 4, 1.0, 1, false);
        let lines = groom::curves(&curves);
        assert_eq!(lines.len(), sim(now).vertices.len() - 1, "the newborn has no past yet");
        let first = &lines[0];
        assert_eq!(first.len(), 4);
        // Each particle rises over time, so its trail runs downward from its newest position.
        assert!(curves.vertices[first.start].y > curves.vertices[first.end - 1].y);
        // Velocity: particle i rises at 1 + i % 3 units a second.
        let moving = trail(sim, now, 2, 1.0, 2, false);
        let v = &moving.point_attributes["v"];
        let ids = &moving.point_attributes["id"];
        for i in 0..moving.vertices.len() - 1 {
            assert!((v[i * 3 + 1] - (1.0 + (ids[i] as i32 % 3) as f32)).abs() < 1e-3, "{i}: {}", v[i * 3 + 1]);
        }
    }

    #[test]
    fn best_length_finds_where_the_end_matches_the_start() {
        // Periodic every 0.75 s (18 frames), and different at every moment of a cycle.
        let periodic = |t: f32| smoke((t / 0.75).fract() * 2.0);
        let (length, diff) = best_length(periodic, 1.0, 0.5, 1.2, 1.0);
        assert!((length - 0.75).abs() < 1e-4 && diff < 1e-3, "{length} {diff}");
        // Something that only drifts away has no dip: the length asked for stays.
        assert_eq!(best_length(smoke, 1.0, 0.1, 0.3, 0.25).0, 0.25);
        assert_eq!((own_weight(0.0, 0.5), own_weight(0.5, 0.5), own_weight(0.3, 0.0)), (0.0, 1.0, 1.0));
    }
}
