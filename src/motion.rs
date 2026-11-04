// Motion vectors for flipbooks: how each pixel of a frame moves to reach the next frame,
// found from the rendered pictures themselves (optical flow), so it works for smoke and
// fire as well as solid things. It does best with smoke, explosions and anything that
// moves smoothly; sparse sparks that move further than the gaps between them can't be
// told apart from frame to frame. A game's shader then slides each frame part of the way
// towards the next instead of cross-fading, so a sheet with few frames plays smoothly.
//
// The flow is coarse-to-fine Lucas–Kanade: on a pyramid of half-size copies, starting from
// the smallest, each level refines the flow from the one below by repeatedly warping the
// next frame back by the flow and solving, in a small window around every pixel, for the
// shift that best lines the two up. A small damping term keeps the flow at zero where the
// picture has no detail to follow, and smoothing between steps spreads it from where it's
// sure into where it isn't.
//
// Brightness is followed through premultiplied colour plus coverage, so a fading wisp and
// an opaque edge both count.

use eframe::egui;
use rayon::prelude::*;

// A greyscale picture.
#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Plane {
    fn new(w: usize, h: usize) -> Plane {
        Plane { w, h, data: vec![0.0; w * h] }
    }

    fn at(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[y * self.w + x]
    }

    // Bilinear, clamped at the edges.
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let top = self.at(x0, y0) * (1.0 - fx) + self.at(x0 + 1, y0) * fx;
        let bottom = self.at(x0, y0 + 1) * (1.0 - fx) + self.at(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    // Half the size, each pixel the average of four.
    fn half(&self) -> Plane {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let mut out = Plane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (2 * x as isize, 2 * y as isize);
                out.data[y * w + x] = (self.at(sx, sy) + self.at(sx + 1, sy) + self.at(sx, sy + 1) + self.at(sx + 1, sy + 1)) * 0.25;
            }
        }
        out
    }

    // Each pixel the average of the (2r+1)² square around it.
    fn box_blur(&self, r: usize) -> Plane {
        let r = r as isize;
        let mut across = Plane::new(self.w, self.h);
        for y in 0..self.h {
            for x in 0..self.w {
                let mut sum = 0.0;
                for d in -r..=r {
                    sum += self.at(x as isize + d, y as isize);
                }
                across.data[y * self.w + x] = sum;
            }
        }
        let mut out = Plane::new(self.w, self.h);
        let n = ((2 * r + 1) * (2 * r + 1)) as f32;
        for y in 0..self.h {
            for x in 0..self.w {
                let mut sum = 0.0;
                for d in -r..=r {
                    sum += across.at(x as isize, y as isize + d);
                }
                out.data[y * self.w + x] = sum / n;
            }
        }
        out
    }
}

// What the flow follows in a frame: premultiplied brightness plus coverage.
pub fn plane(frame: &egui::ColorImage) -> Plane {
    let [w, h] = frame.size;
    let data = frame
        .pixels
        .iter()
        .map(|c| {
            let [r, g, b, a] = c.to_array().map(|v| v as f32 / 255.0);
            0.3 * r + 0.59 * g + 0.11 * b + a
        })
        .collect();
    Plane { w, h, data }
}

// How each pixel moves, in pixels: a(x, y) ≈ b(x + u, y + v).
#[derive(Clone, Debug)]
pub struct Flow {
    pub w: usize,
    pub h: usize,
    pub u: Vec<f32>,
    pub v: Vec<f32>,
}

impl Flow {
    pub fn zero(w: usize, h: usize) -> Flow {
        Flow { w, h, u: vec![0.0; w * h], v: vec![0.0; w * h] }
    }

    #[cfg_attr(not(test), allow(dead_code))] // only the tests call it
    pub fn at(&self, x: usize, y: usize) -> (f32, f32) {
        (self.u[y * self.w + x], self.v[y * self.w + x])
    }

    // Twice the size (or to `w`×`h`), with the shifts doubled to match.
    fn upsample(&self, w: usize, h: usize) -> Flow {
        let scale = |p: &[f32]| {
            let plane = Plane { w: self.w, h: self.h, data: p.to_vec() };
            let (sx, sy) = (self.w as f32 / w as f32, self.h as f32 / h as f32);
            let mut out = vec![0.0; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = plane.sample((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5);
                }
            }
            out
        };
        let (mx, my) = (w as f32 / self.w as f32, h as f32 / self.h as f32);
        Flow { w, h, u: scale(&self.u).into_iter().map(|u| u * mx).collect(), v: scale(&self.v).into_iter().map(|v| v * my).collect() }
    }

    // Averages each pixel's shift with its neighbours', weighting each by how much detail
    // it had to follow (`weight`), so sure shifts spread into unsure ones rather than the
    // other way round.
    fn smooth(&mut self, weight: &Plane, r: usize) {
        let total = weight.box_blur(r);
        let spread = |p: &[f32]| {
            let weighted = Plane { w: self.w, h: self.h, data: p.iter().zip(&weight.data).map(|(x, w)| x * w).collect() }.box_blur(r);
            weighted.data.iter().zip(&total.data).map(|(x, w)| if *w > 1e-9 { x / w } else { 0.0 }).collect::<Vec<f32>>()
        };
        self.u = spread(&self.u);
        self.v = spread(&self.v);
    }
}

// Refines `flow` from `a` to `b` (same size) by Lucas–Kanade steps.
fn refine(a: &Plane, b: &Plane, flow: &mut Flow, steps: usize) {
    let (w, h) = (a.w, a.h);
    // Damping: about a hundredth of a strong edge's squared gradient, summed over the window.
    const DAMPING: f32 = 2e-4;
    for _ in 0..steps {
        let mut warped = Plane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                warped.data[i] = b.sample(x as f32 + flow.u[i], y as f32 + flow.v[i]);
            }
        }
        // Gradients of both pictures, averaged, and their difference.
        let (mut xx, mut xy, mut yy, mut xt, mut yt) = (Plane::new(w, h), Plane::new(w, h), Plane::new(w, h), Plane::new(w, h), Plane::new(w, h));
        for y in 0..h as isize {
            for x in 0..w as isize {
                let i = y as usize * w + x as usize;
                let gx = 0.25 * (a.at(x + 1, y) - a.at(x - 1, y) + warped.at(x + 1, y) - warped.at(x - 1, y));
                let gy = 0.25 * (a.at(x, y + 1) - a.at(x, y - 1) + warped.at(x, y + 1) - warped.at(x, y - 1));
                let gt = warped.data[i] - a.data[i];
                xx.data[i] = gx * gx;
                xy.data[i] = gx * gy;
                yy.data[i] = gy * gy;
                xt.data[i] = gx * gt;
                yt.data[i] = gy * gt;
            }
        }
        let [xx, xy, yy, xt, yt] = [xx, xy, yy, xt, yt].map(|p| p.box_blur(2));
        // How much detail each window has: its smaller eigenvalue, so an edge (which can
        // only say how far it moved across itself) counts for less than a corner or blob.
        let mut sure = Plane::new(w, h);
        for i in 0..w * h {
            let (a11, a12, a22) = (xx.data[i] + DAMPING, xy.data[i], yy.data[i] + DAMPING);
            let det = a11 * a22 - a12 * a12;
            // Solve [a11 a12; a12 a22] d = -[xt; yt]; at most a pixel a step, for stability.
            let du = (-(a22 * xt.data[i] - a12 * yt.data[i]) / det).clamp(-1.0, 1.0);
            let dv = (-(a11 * yt.data[i] - a12 * xt.data[i]) / det).clamp(-1.0, 1.0);
            flow.u[i] += du;
            flow.v[i] += dv;
            let (tr, d) = (xx.data[i] + yy.data[i], (xx.data[i] * yy.data[i] - xy.data[i] * xy.data[i]).max(0.0));
            sure.data[i] = 0.5 * tr - (0.25 * tr * tr - d).max(0.0).sqrt() + 1e-7;
        }
        flow.smooth(&sure, 2);
    }
}

// A first guess for big moves, on the smallest level: for every pixel, the whole-pixel
// shift up to `range` that best lines up the window around it, a little in favour of
// shorter shifts so empty areas stay still.
fn search(a: &Plane, b: &Plane, range: isize) -> Flow {
    let (w, h) = (a.w, a.h);
    let mut costs = Vec::new();
    let mut shifts = Vec::new();
    for dy in -range..=range {
        for dx in -range..=range {
            let mut diff = Plane::new(w, h);
            for y in 0..h as isize {
                for x in 0..w as isize {
                    let d = b.at(x + dx, y + dy) - a.at(x, y);
                    diff.data[y as usize * w + x as usize] = d * d;
                }
            }
            costs.push(diff.box_blur(2));
            shifts.push((dx, dy));
        }
    }
    let still = shifts.iter().position(|&d| d == (0, 0)).expect("the search includes no shift");
    let mut flow = Flow::zero(w, h);
    for i in 0..w * h {
        // A shift has to line things up clearly better than none, and longer ones more so:
        // where any shift fits about as well (along an edge, or a stripe running off the
        // frame), the picture isn't saying it moved.
        let base = costs[still].data[i];
        let cost = |k: usize| costs[k].data[i] * (1.0 + 0.05 * (shifts[k].0.pow(2) + shifts[k].1.pow(2)) as f32);
        let best = (0..shifts.len()).min_by(|&p, &q| cost(p).total_cmp(&cost(q))).unwrap_or(still);
        let best = if costs[best].data[i] < 0.7 * base { best } else { still };
        flow.u[i] = shifts[best].0 as f32;
        flow.v[i] = shifts[best].1 as f32;
    }
    flow
}

// The flow from `a` to `b`, which must be the same size.
pub fn flow(a: &Plane, b: &Plane) -> Flow {
    let mut pyramid = vec![(a.clone(), b.clone())];
    while let Some((pa, pb)) = pyramid.last() {
        if pa.w.min(pa.h) < 24 || pyramid.len() >= 7 {
            break;
        }
        let next = (pa.half(), pb.half());
        pyramid.push(next);
    }
    let mut result: Option<Flow> = None;
    for (pa, pb) in pyramid.iter().rev() {
        let mut f = match result {
            None => search(pa, pb, 4),
            Some(coarse) => coarse.upsample(pa.w, pa.h),
        };
        refine(pa, pb, &mut f, 6);
        result = Some(f);
    }
    let mut f = result.expect("at least one level");
    keep_what_helps(a, b, &mut f);
    f
}

// Fades the flow out where it doesn't line the frames up clearly better than no motion
// would: flickering fire, say, whose outline stays put while its detail changes. There
// a game's shader is better off cross-fading, which is what no motion does.
fn keep_what_helps(a: &Plane, b: &Plane, flow: &mut Flow) {
    let (w, h) = (a.w, a.h);
    let (mut moved, mut still) = (Plane::new(w, h), Plane::new(w, h));
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            moved.data[i] = (b.sample(x as f32 + flow.u[i], y as f32 + flow.v[i]) - a.data[i]).abs();
            still.data[i] = (b.data[i] - a.data[i]).abs();
        }
    }
    let (moved, still) = (moved.box_blur(4), still.box_blur(4));
    let mut keep = Plane::new(w, h);
    for i in 0..w * h {
        // Kept whole where it takes away at least a third of the mismatch.
        let gain = (still.data[i] - moved.data[i]) / still.data[i].max(1e-4);
        keep.data[i] = (gain / 0.33).clamp(0.0, 1.0);
    }
    let keep = keep.box_blur(2);
    for i in 0..w * h {
        flow.u[i] *= keep.data[i];
        flow.v[i] *= keep.data[i];
    }
}

// The flow from `a` to `b`, kept only where the flow back from `b` to `a` agrees: real
// motion reads the same both ways, while flicker and things appearing or vanishing don't.
pub fn consistent_flow(a: &Plane, b: &Plane) -> Flow {
    let mut forward = flow(a, b);
    let back = flow(b, a);
    let (bu, bv) = (Plane { w: back.w, h: back.h, data: back.u }, Plane { w: back.w, h: back.h, data: back.v });
    let mut keep = Plane::new(forward.w, forward.h);
    for y in 0..forward.h {
        for x in 0..forward.w {
            let i = y * forward.w + x;
            let (u, v) = (forward.u[i], forward.v[i]);
            // Going there and back again should end where it started.
            let (tx, ty) = (x as f32 + u, y as f32 + v);
            let miss = ((u + bu.sample(tx, ty)).powi(2) + (v + bv.sample(tx, ty)).powi(2)).sqrt();
            let size = (u * u + v * v).sqrt();
            // Allowed to miss by half a pixel, and a quarter of the distance moved.
            keep.data[i] = (1.0 - (miss - 0.5 - 0.25 * size).max(0.0)).clamp(0.0, 1.0);
        }
    }
    let keep = keep.box_blur(2);
    for i in 0..keep.data.len() {
        forward.u[i] *= keep.data[i];
        forward.v[i] *= keep.data[i];
    }
    forward
}

// Whether the frames look like a loop: the last leads into the first about as gently as
// each frame leads into the next.
pub fn looks_looped(frames: &[egui::ColorImage]) -> bool {
    if frames.len() < 3 {
        return false;
    }
    let change = |a: &egui::ColorImage, b: &egui::ColorImage| -> f32 {
        a.pixels.iter().zip(&b.pixels).map(|(p, q)| p.to_array().iter().zip(q.to_array()).map(|(x, y)| (*x as f32 - y as f32).abs()).sum::<f32>()).sum::<f32>() / a.pixels.len().max(1) as f32
    };
    let mut steps: Vec<f32> = frames.windows(2).map(|w| change(&w[0], &w[1])).collect();
    steps.sort_by(f32::total_cmp);
    let typical = steps[steps.len() / 2];
    change(&frames[frames.len() - 1], &frames[0]) <= typical * 1.5 + 0.5
}

// Each frame's flow to the frame after it. The last frame's leads into the first when the
// frames loop; otherwise it repeats the one before (there's nothing after it to reach).
pub fn flows(frames: &[egui::ColorImage], looping: bool) -> Vec<Flow> {
    let planes: Vec<Plane> = frames.par_iter().map(plane).collect();
    let n = planes.len();
    if n < 2 {
        return planes.iter().map(|p| Flow::zero(p.w, p.h)).collect();
    }
    let pairs = if looping { n } else { n - 1 };
    let mut out: Vec<Flow> = (0..pairs).into_par_iter().map(|i| consistent_flow(&planes[i], &planes[(i + 1) % n])).collect();
    if !looping {
        out.push(out[n - 2].clone());
    }
    out
}

// The flows as pictures for the motion sheet, and the scale they're stored at. Each pixel's
// red and green hold its shift across and down in fractions of the frame `cell` it's packed
// into (after trimming): 0.5 is still, and 0 and 1 are `strength` back and forward. Blue is
// 0 and alpha is opaque.
pub fn encode(flows: &[Flow], cell: [u32; 2]) -> (Vec<egui::ColorImage>, f32) {
    let (cw, ch) = (cell[0].max(1) as f32, cell[1].max(1) as f32);
    let largest = flows.iter().flat_map(|f| f.u.iter().map(move |u| u.abs() / cw).chain(f.v.iter().map(move |v| v.abs() / ch))).fold(0.0f32, f32::max);
    // Rounded up, so the JSON's number is short and the extremes still fit.
    let strength = ((largest * 1000.0).ceil() / 1000.0).max(0.001);
    let to_byte = |shift: f32| ((0.5 + 0.5 * shift / strength).clamp(0.0, 1.0) * 255.0).round() as u8;
    let images = flows
        .iter()
        .map(|f| egui::ColorImage {
            size: [f.w, f.h],
            source_size: egui::vec2(f.w as f32, f.h as f32),
            pixels: f.u.iter().zip(&f.v).map(|(u, v)| egui::Color32::from_rgba_premultiplied(to_byte(u / cw), to_byte(v / ch), 0, 255)).collect(),
        })
        .collect();
    (images, strength)
}

// The picture `t` of the way (0 to 1) from `a` to `b`, the way the engines' shaders draw it:
// `a` slid forward along the flow, `b` slid back, blended. For tests and previews.
#[cfg_attr(not(test), allow(dead_code))] // only the tests call it
pub fn between(a: &egui::ColorImage, b: &egui::ColorImage, flow: &Flow, t: f32) -> egui::ColorImage {
    let [w, h] = a.size;
    let channel = |img: &egui::ColorImage, k: usize| Plane { w, h, data: img.pixels.iter().map(|c| c.to_array()[k] as f32).collect() };
    let (ca, cb): (Vec<Plane>, Vec<Plane>) = ((0..4).map(|k| channel(a, k)).collect(), (0..4).map(|k| channel(b, k)).collect());
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = flow.at(x, y);
            let (xa, ya) = (x as f32 - u * t, y as f32 - v * t);
            let (xb, yb) = (x as f32 + u * (1.0 - t), y as f32 + v * (1.0 - t));
            let px: [u8; 4] = std::array::from_fn(|k| (ca[k].sample(xa, ya) * (1.0 - t) + cb[k].sample(xb, yb) * t).round().clamp(0.0, 255.0) as u8);
            pixels.push(egui::Color32::from_rgba_premultiplied(px[0], px[1], px[2], px[3]));
        }
    }
    egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A soft white blob at (cx, cy) on transparent.
    fn blob(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> egui::ColorImage {
        let pixels = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let d2 = ((x - cx).powi(2) + (y - cy).powi(2)) / (r * r);
                let a = ((-d2).exp() * 255.0).round() as u8;
                egui::Color32::from_rgba_premultiplied(a, a / 2, a / 4, a)
            })
            .collect();
        egui::ColorImage { size: [w, h], source_size: egui::vec2(w as f32, h as f32), pixels }
    }

    fn difference(a: &egui::ColorImage, b: &egui::ColorImage) -> f32 {
        a.pixels.iter().zip(&b.pixels).map(|(p, q)| p.to_array().iter().zip(q.to_array()).map(|(x, y)| (*x as f32 - y as f32).abs()).sum::<f32>()).sum::<f32>() / a.pixels.len() as f32
    }

    #[test]
    fn the_flow_follows_a_moving_blob() {
        let (a, b) = (blob(96, 96, 40.0, 50.0, 9.0), blob(96, 96, 46.0, 46.0, 9.0));
        let f = flow(&plane(&a), &plane(&b));
        // Over the blob, weighted by how much of it each pixel shows.
        let (mut su, mut sv, mut total) = (0.0, 0.0, 0.0);
        for (i, c) in a.pixels.iter().enumerate() {
            let weight = c.a() as f32;
            su += f.u[i] * weight;
            sv += f.v[i] * weight;
            total += weight;
        }
        let (u, v) = (su / total, sv / total);
        assert!((u - 6.0).abs() < 0.5 && (v + 4.0).abs() < 0.5, "({u}, {v})");
        // Empty space takes the motion of what's nearest (it has nothing to slide, so it
        // makes no difference), and a still picture doesn't move.
        let still = flow(&plane(&a), &plane(&a));
        assert!(still.u.iter().chain(&still.v).all(|d| d.abs() < 0.05), "a still picture stays still");
    }

    #[test]
    fn in_betweens_match_the_real_middle_frame_better_than_a_cross_fade() {
        let (a, mid, b) = (blob(96, 96, 30.0, 48.0, 8.0), blob(96, 96, 42.0, 44.0, 8.0), blob(96, 96, 54.0, 40.0, 8.0));
        let f = flow(&plane(&a), &plane(&b));
        let slid = between(&a, &b, &f, 0.5);
        let faded = between(&a, &b, &Flow::zero(96, 96), 0.5);
        let (slid, faded) = (difference(&slid, &mid), difference(&faded, &mid));
        assert!(slid < faded * 0.3, "sliding is off by {slid} a pixel, fading by {faded}");
    }

    #[test]
    fn encoding_keeps_still_at_the_middle_and_the_largest_shift_at_the_edge() {
        let mut f = Flow::zero(4, 2);
        f.u[1] = 8.0; // a fifth of a 40-pixel-wide cell
        f.v[2] = -2.0; // a tenth of a 20-pixel-high cell
        let (images, strength) = encode(&[f], [40, 20]);
        assert_eq!(strength, 0.2);
        let px = |i: usize| images[0].pixels[i].to_array();
        assert_eq!(px(0), [128, 128, 0, 255]);
        assert_eq!(px(1)[0], 255);
        assert_eq!(px(2)[1], 64);
    }

    #[test]
    fn loops_are_recognised() {
        let circle: Vec<_> = (0..8).map(|i| { let t = i as f32 / 8.0 * std::f32::consts::TAU; blob(48, 48, 24.0 + 8.0 * t.cos(), 24.0 + 8.0 * t.sin(), 6.0) }).collect();
        assert!(looks_looped(&circle));
        let line: Vec<_> = (0..8).map(|i| blob(64, 48, 8.0 + 6.0 * i as f32, 24.0, 6.0)).collect();
        assert!(!looks_looped(&line));
    }

    #[test]
    fn loops_lead_back_into_the_first_frame() {
        let frames: Vec<_> = (0..3).map(|i| blob(48, 48, 16.0 + 6.0 * i as f32, 24.0, 6.0)).collect();
        let looped = flows(&frames, true);
        let (u, _) = looped[2].at(28, 24);
        assert!(u < -4.0, "the last frame's flow runs back to the start: {u}");
        let once = flows(&frames, false);
        assert_eq!(once[2].u, once[1].u, "without a loop, the last frame repeats the one before");
    }
}

