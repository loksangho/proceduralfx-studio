// Pack and Unpack: geometry as lightweight packed points.
//
// Pack turns each piece (or the whole input) into one point: its pivot as P,
// a `transform` (3x3, rows are the piece's axes, scale included), `piece`,
// and `packed:id`, a reference to the piece's geometry kept once in a shared
// store. Packed points can be moved, rotated, copied, merged and simulated
// cheaply; Unpack (and the viewport and renderer, automatically) turns them
// back into real geometry at their current transform: p * transform + P,
// with `orient` and `pscale` applied when there's no `transform`.

use super::{Mesh, Vec3};
use crate::kinefx::{vm, IDENTITY, M3};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub const ID: &str = "packed:id";
pub const MODES: [&str; 3] = ["Whole input", "Each piece (piece or name)", "Connected pieces"];
pub const PIVOTS: [&str; 2] = ["Centre", "Origin"];

#[derive(Clone, Debug, PartialEq)]
pub struct PackSettings {
    pub mode: usize,
    pub pivot: usize,
}

impl PackSettings {
    pub fn defaults() -> Self {
        PackSettings { mode: 1, pivot: 0 }
    }
}

// The packed geometry store: entries by index, and a content hash to reuse them.
#[derive(Default)]
struct Store {
    meshes: Vec<Arc<Mesh>>,
    by_hash: HashMap<u64, usize>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

fn content_hash(mesh: &Mesh) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    mesh.vertices.iter().for_each(|v| [v.x, v.y, v.z].map(f32::to_bits).hash(&mut h));
    mesh.indices.hash(&mut h);
    let mut keys: Vec<&String> = mesh.point_attributes.keys().collect();
    keys.sort();
    for k in keys {
        k.hash(&mut h);
        mesh.point_attributes[k].iter().for_each(|v| v.to_bits().hash(&mut h));
    }
    h.finish()
}

fn put(mesh: Mesh) -> usize {
    let hash = content_hash(&mesh);
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&i) = s.by_hash.get(&hash) {
        return i;
    }
    s.meshes.push(Arc::new(mesh));
    let i = s.meshes.len() - 1;
    s.by_hash.insert(hash, i);
    i
}

fn get(id: usize) -> Option<Arc<Mesh>> {
    store().lock().unwrap_or_else(|e| e.into_inner()).meshes.get(id).cloned()
}

pub fn is_packed(mesh: &Mesh) -> bool {
    mesh.point_attributes.get(ID).is_some_and(|v| v.len() == mesh.vertices.len() && !v.is_empty())
}

pub fn pack(input: &Mesh, set: &PackSettings) -> Result<Mesh, String> {
    if input.vertices.is_empty() {
        return Err("nothing to pack".into());
    }
    let pieces: Vec<Mesh> = match set.mode {
        0 => vec![input.clone()],
        mode => {
            let has_name = ["piece", "name"].iter().find(|n| input.point_attributes.contains_key(**n) || input.point_attributes.contains_key(&format!("prim:{n}")));
            let fe = match (mode, has_name) {
                (1, Some(name)) => crate::flow::ForEachSettings { attribute: (*name).to_owned(), ..crate::flow::ForEachSettings::new(crate::flow::NAMED) },
                (1, None) => return Err("the input has no `piece` or `name` attribute: pack the whole input, or connected pieces".into()),
                _ => crate::flow::ForEachSettings::new(crate::flow::CONNECTED),
            };
            crate::flow::pieces(input, &fe)?
        }
    };
    let mut out = Mesh::default();
    let (mut ids, mut transforms, mut piece_ids) = (Vec::new(), Vec::new(), Vec::new());
    for (i, mut piece) in pieces.into_iter().enumerate() {
        let pivot = if set.pivot == 1 {
            Vec3::zero()
        } else {
            piece.bounds().map_or(Vec3::zero(), |(lo, hi)| lo.add(hi).scale(0.5))
        };
        piece.vertices.iter_mut().for_each(|v| *v = v.sub(pivot));
        piece.point_attributes.retain(|k, _| !k.starts_with("detail:"));
        ids.push(put(piece) as f32);
        out.vertices.push(pivot);
        transforms.extend([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        piece_ids.push(i as f32);
    }
    out.point_attributes.insert(ID.into(), ids);
    out.point_attributes.insert("transform".into(), transforms);
    out.point_attributes.insert("piece".into(), piece_ids);
    Ok(out)
}

// Each packed point's frame: `transform`, else `orient` and `pscale`.
fn frame(points: &Mesh, i: usize) -> M3 {
    let n = points.vertices.len();
    if let Some(t) = points.point_attributes.get("transform").filter(|t| t.len() == n * 9) {
        let c = &t[i * 9..i * 9 + 9];
        return [Vec3::new(c[0], c[1], c[2]), Vec3::new(c[3], c[4], c[5]), Vec3::new(c[6], c[7], c[8])];
    }
    let mut m = match points.point_attributes.get("orient").filter(|o| o.len() == n * 4) {
        Some(o) => {
            let q = crate::vexmath::q_to_m3(&[o[i * 4], o[i * 4 + 1], o[i * 4 + 2], o[i * 4 + 3]]);
            [Vec3::new(q[0], q[1], q[2]), Vec3::new(q[3], q[4], q[5]), Vec3::new(q[6], q[7], q[8])]
        }
        None => IDENTITY,
    };
    if let Some(s) = points.point_attributes.get("pscale").filter(|s| s.len() == n) {
        m = m.map(|r| r.scale(s[i]));
    }
    m
}

// Real geometry for packed points (other points pass through). `transfer`
// copies the packed points' own attributes (Cd, piece…) onto their geometry.
pub fn unpack(input: &Mesh, transfer: bool) -> Mesh {
    if !is_packed(input) {
        return input.clone();
    }
    let ids = &input.point_attributes[ID];
    let n = input.vertices.len();
    let skip = |k: &str| matches!(k, ID | "transform" | "orient" | "pscale" | "P") || k.starts_with("prim:") || k.starts_with("vertex:") || k.starts_with("detail:");
    let mut out = Mesh::default();
    for i in 0..n {
        let Some(geo) = get(ids[i].round().max(0.0) as usize) else { continue };
        let m = frame(input, i);
        let mut piece = (*geo).clone();
        piece.vertices.iter_mut().for_each(|v| *v = vm(*v, &m).add(input.vertices[i]));
        if m[0].cross(m[1]).dot(m[2]) < 0.0 {
            piece.indices.iter_mut().for_each(|t| t.swap(1, 2));
        }
        piece.point_attributes.remove("N");
        if transfer {
            for (key, values) in &input.point_attributes {
                if skip(key) || values.len() % n != 0 {
                    continue;
                }
                let w = values.len() / n;
                piece.point_attributes.insert(key.clone(), values[i * w..i * w + w].iter().copied().cycle().take(piece.vertices.len() * w).collect());
            }
        }
        out = crate::node_merge(out, piece);
    }
    out
}

// Packed points turned or scaled by a transform: their frames turn too (row vectors: T * m).
pub fn turn_frames(mesh: &mut Mesh, m: &M3) {
    let n = mesh.vertices.len();
    if let Some(t) = mesh.point_attributes.get_mut("transform").filter(|t| t.len() == n * 9) {
        for c in t.chunks_mut(9) {
            let rows = [Vec3::new(c[0], c[1], c[2]), Vec3::new(c[3], c[4], c[5]), Vec3::new(c[6], c[7], c[8])];
            let r = crate::kinefx::mm(&rows, m);
            c.copy_from_slice(&[r[0].x, r[0].y, r[0].z, r[1].x, r[1].y, r[1].z, r[2].x, r[2].y, r[2].z]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_pieces() -> Mesh {
        let mut a = crate::node_grid(1.0, 1.0, 2, 2);
        let mut b = a.clone();
        b.vertices.iter_mut().for_each(|v| *v = v.add(Vec3::new(5.0, 0.0, 0.0)));
        a.point_attributes.insert("piece".into(), vec![0.0; 4]);
        b.point_attributes.insert("piece".into(), vec![1.0; 4]);
        crate::node_merge(a, b)
    }

    #[test]
    fn pack_and_unpack_round_trip() {
        let geo = two_pieces();
        let packed = pack(&geo, &PackSettings::defaults()).unwrap();
        assert_eq!(packed.vertices.len(), 2);
        assert!(packed.indices.is_empty() && is_packed(&packed));
        assert!(packed.vertices[1].sub(Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5, "pivot at the piece centre");
        let back = unpack(&packed, true);
        assert_eq!(back.indices.len(), geo.indices.len());
        let mut a: Vec<_> = back.vertices.iter().map(|v| ((v.x * 100.0) as i32, (v.z * 100.0) as i32)).collect();
        let mut b: Vec<_> = geo.vertices.iter().map(|v| ((v.x * 100.0) as i32, (v.z * 100.0) as i32)).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        // The same piece geometry is stored once.
        let mut same = geo.clone();
        same.point_attributes.remove("piece");
        let ids = &pack(&same, &PackSettings { mode: 2, pivot: 0 }).unwrap().point_attributes[ID];
        assert_eq!(ids[0], ids[1], "identical pieces share their stored geometry");
        // Moving and turning a packed point moves and turns its piece.
        let mut moved = packed.clone();
        moved.vertices[0] = Vec3::new(0.0, 10.0, 0.0);
        let turn = crate::kinefx::euler([0.0, 0.0, 90.0]);
        turn_frames(&mut moved, &turn);
        let out = unpack(&moved, true);
        let top = out.vertices.iter().map(|v| v.y).fold(f32::MIN, f32::max);
        assert!((top - 10.5).abs() < 1e-4, "the square stands up around (0, 10, 0): top {top}");
        assert!(out.point_attributes["piece"].iter().any(|p| *p == 1.0));
        assert!(pack(&crate::node_grid(1.0, 1.0, 2, 2), &PackSettings::defaults()).is_err(), "no piece attribute");
        assert_eq!(pack(&crate::node_grid(1.0, 1.0, 2, 2), &PackSettings { mode: 0, pivot: 1 }).unwrap().vertices, vec![Vec3::zero()]);
    }
}
