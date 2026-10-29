#!/usr/bin/env python3
# Writes the starter effect presets (assets/presets/*.pfs) from the readable sources below:
# .pfs files store wrangle code hex-encoded. Run it after changing a preset here:
#
#     python3 assets/presets/make_presets.py
#
# src/presets.rs lists the presets with their playback range, frame size and looping.
import pathlib

out = pathlib.Path(__file__).resolve().parent

def hx(s):
    return s.encode().hex()

def camera(cid, target, zoom):
    tx, ty, tz = target
    return f"node {cid} 40 -80 0 obj camera {tx} {ty} {tz} 0 0 0 1.2 0.5 1 0 0 {zoom}"

def pyro_scene(name, sphere, pyro, cam_target, zoom):
    lines = [
        "pfs 2",
        "node 0 40 20 0 obj geometry -1",
        f"node 1 40 20 0 obj sphere {sphere}",
        f"node 2 40 100 0 obj pyro {pyro}",
        camera(3, cam_target, zoom),
        "link 1 2 0",
        "parent 1 0",
        "parent 2 0",
        f"name 0 {hx(name)}",
        f"name 1 {hx('source')}",
        f"name 2 {hx(name + '_sim')}",
        f"name 3 {hx('sheet_camera')}",
        "display 0 2",
        "next 4",
    ]
    return "\n".join(lines) + "\n"

def points_scene(name, code, chans, cam_target, zoom):
    lines = [
        "pfs 2",
        "node 0 40 20 0 obj geometry -1",
        f"node 1 40 20 0 obj wrangle 0:{hx(code.strip())}",
        camera(2, cam_target, zoom),
        "parent 1 0",
    ]
    for cname, value in chans:
        lines.append(f"chan 1 {hx(cname)} {value} 0 0")
    lines += [
        f"name 0 {hx(name)}",
        f"name 1 {hx(name + '_points')}",
        f"name 2 {hx('sheet_camera')}",
        "display 0 1",
        "next 3",
    ]
    return "\n".join(lines) + "\n"

# Pyro parameters: resolution, center xyz, size xyz, substeps, density, temperature, fuel,
# buoyancy, dissipation, cooling, vorticity, turbulence, burn_rate, wind xyz, opacity.

SMOKE_CODE = r"""
// Rising smoke: soft puffs drift up, spread, sway and fade, over and over.
// Each puff lives two loops; puffs come in pairs half a life apart, so the
// smoke one loop later is exactly the smoke now, and the sheet loops cleanly.
float t = @Time;
float loop = ch("loop");
int pairs = int(ch("puffs"));
float rise = ch("rise");
float spread = ch("spread");
float size = ch("size");
for (int i = 0; i < pairs * 2; i++) {
    int id = i % pairs;
    float k = frac(t / (2 * loop) + i / (2.0 * pairs));
    float side = (rand(id + 0.7) - 0.5) * 0.25 + (rand(id + 0.5) - 0.5) * spread * k;
    float sway = sin(k * 6.28318 + rand(id + 0.3) * 6.28318) * 0.12 * k;
    vector p = set(side + sway, rise * k * fit(rand(id + 0.1), 0, 1, 0.8, 1.1), (rand(id + 0.9) - 0.5) * 0.2);
    int pt = addpoint(0, p);
    setpointattrib(0, "Alpha", pt, min(k * 6, 1) * pow(1 - k, 1.5) * 0.55);
    setpointattrib(0, "pscale", pt, size * (1 + k * 2.5) * fit(rand(id + 0.4), 0, 1, 0.8, 1.2));
    float grey = lerp(0.6, 0.38, k);
    setpointattrib(0, "Cd", pt, set(grey, grey, grey * 1.04));
}
"""

FIRE = "56 0 1.2 0 3 4 3 1 0.03 0.4 1 1.3 2 1.8 0.4 0.3 1.2 0 0 0 1"
SMOKE = "64 0 1.5 0 3 4 3 1 0.8 1 0 0.6 0.2 0.8 0.5 0.12 1 0 0 0 1"

IMPACT = r"""
// An impact: a quick flash, and a fan of sparks that fly out, slow, fall and fade.
// Glowing points with Alpha, Cd, pscale and emission render as soft splats.
float t = @Time;
int count = int(ch("count"));
float speed = ch("speed");
float life = ch("life");
float size = ch("size");
for (int i = 0; i < count; i++) {
    float age = t - rand(i + 0.17) * 0.04;
    float span = life * fit(rand(i + 0.53), 0, 1, 0.5, 1);
    if (age < 0 || age > span) continue;
    float k = age / span;
    // Mostly upward and outward, as from a hit on the ground.
    float a = fit(rand(i + 0.91), 0, 1, -0.35, 3.49);
    vector dir = set(cos(a), sin(a), (rand(i + 0.33) - 0.5) * 0.3);
    float v = speed * fit(rand(i + 0.71), 0, 1, 0.35, 1);
    // Each spark is a short streak: its position now and a moment ago.
    for (int s = 0; s < 10; s++) {
        float b = max(age - s * 0.005, 0);
        vector p = dir * v * (1 - exp(-3 * b)) / 3 - set(0, 2.5 * b * b, 0);
        int pt = addpoint(0, p);
        float fade = pow(1 - k, 0.6) * (1 - s * 0.09);
        setpointattrib(0, "Alpha", pt, fade);
        setpointattrib(0, "pscale", pt, size * (1 - s * 0.06));
        setpointattrib(0, "Cd", pt, lerp(set(1, 0.95, 0.75), set(1, 0.45, 0.1), k));
        setpointattrib(0, "emission", pt, set(5, 2.4, 0.7) * fade);
    }
}
// The flash where it hit.
float flash = max(1 - t / 0.12, 0);
if (flash > 0) {
    int c = addpoint(0, set(0, 0, 0));
    setpointattrib(0, "Alpha", c, flash);
    setpointattrib(0, "pscale", c, 0.35 + (1 - flash) * 0.3);
    setpointattrib(0, "Cd", c, set(1, 0.9, 0.7));
    setpointattrib(0, "emission", c, set(6, 4, 2) * flash);
}
"""

MAGIC = r"""
// A magic burst: motes spiral out from a swelling core and fade.
float t = @Time;
int count = int(ch("count"));
float radius = ch("radius");
float dur = ch("duration");
float swirl = ch("swirl");
for (int i = 0; i < count; i++) {
    float delay = rand(i + 0.6) * 0.15;
    float k = (t - delay) / (dur - delay);
    if (k <= 0 || k >= 1) continue;
    // Fast at first, easing out.
    float e = 1 - pow(1 - k, 3);
    float a = i * 6.28318 / count + rand(i + 0.2) * 0.3 + e * swirl;
    float r = radius * e * fit(rand(i + 0.4), 0, 1, 0.7, 1.1);
    vector p = set(cos(a) * r, sin(a) * r, (rand(i + 0.8) - 0.5) * 0.2);
    int pt = addpoint(0, p);
    float fade = (1 - k) * min(k * 8, 1);
    vector col = lerp(set(0.6, 0.35, 1), set(0.3, 0.9, 1), rand(i + 0.9));
    setpointattrib(0, "Alpha", pt, fade);
    setpointattrib(0, "pscale", pt, 0.05 * fit(rand(i + 0.1), 0, 1, 0.6, 1.4));
    setpointattrib(0, "Cd", pt, col);
    setpointattrib(0, "emission", pt, col * 4 * fade);
}
// The core swells, then fades.
float g = sin(clamp(t / (dur * 0.6), 0, 1) * 3.14159);
if (g > 0.01) {
    for (int j = 0; j < 3; j++) {
        int c = addpoint(0, set(0, 0, 0));
        setpointattrib(0, "Alpha", c, g * (1 - j * 0.3));
        setpointattrib(0, "pscale", c, (0.25 + j * 0.2) * (0.6 + g * 0.6));
        setpointattrib(0, "Cd", c, set(0.75, 0.6, 1));
        setpointattrib(0, "emission", c, set(3, 2.5, 6) * g * (1 - j * 0.3));
    }
}
"""

SLASH = r"""
// A sword slash: a bright arc sweeps across, trails off behind its tip, then fades.
float t = @Time;
float dur = ch("duration");
float radius = ch("radius");
float arc = ch("arc") * 0.0174533;
float width = ch("width");
float trail = 0.6;
int n = 160;
float tip = 1 - pow(1 - clamp(t / dur, 0, 1), 2);
float after = clamp(1 - (t - dur) / 0.15, 0, 1);
for (int i = 0; i < n && after > 0; i++) {
    float u = i / (n - 1.0);
    float behind = tip - u;
    if (behind < 0 || behind > trail) continue;
    // 1 at the tip, 0 at the end of the trail.
    float w = 1 - behind / trail;
    // From upper left, clockwise down to the right.
    float ang = 2.5 - u * arc;
    for (int j = 0; j < 5; j++) {
        float across = j / 4.0 - 0.5;
        float rr = radius + across * width * w;
        int pt = addpoint(0, set(cos(ang) * rr, sin(ang) * rr, 0));
        float edge = 1 - abs(across) * 1.2;
        float alpha = w * after * edge;
        vector col = lerp(set(0.55, 0.85, 1), set(1, 1, 1), edge);
        setpointattrib(0, "Alpha", pt, alpha);
        setpointattrib(0, "pscale", pt, 0.04 * (0.5 + w));
        setpointattrib(0, "Cd", pt, col);
        setpointattrib(0, "emission", pt, col * 3 * alpha);
    }
    // A soft cyan glow around the blade.
    if (i % 2 == 0) {
        int g = addpoint(0, set(cos(ang) * radius, sin(ang) * radius, 0));
        setpointattrib(0, "Alpha", g, 0.25 * w * after);
        setpointattrib(0, "pscale", g, width * (0.6 + w));
        setpointattrib(0, "Cd", g, set(0.4, 0.8, 1));
        setpointattrib(0, "emission", g, set(0.6, 1.6, 3) * w * after);
    }
}
"""

files = {
    "fire.pfs": pyro_scene("fire", "0.3 12 24", FIRE, (0, 1.2, 0), 3.8),
    "smoke.pfs": points_scene("smoke", SMOKE_CODE, [("loop", 1), ("puffs", 24), ("rise", 2.4), ("spread", 0.6), ("size", 0.16)], (0, 1.2, 0), 3.4),
    "impact.pfs": points_scene("impact", IMPACT, [("count", 70), ("speed", 9), ("life", 0.45), ("size", 0.03)], (0, 0.5, 0), 3.2),
    "magic_burst.pfs": points_scene("magic_burst", MAGIC, [("count", 160), ("radius", 1.4), ("duration", 0.7), ("swirl", 1.2)], (0, 0, 0), 4),
    "slash.pfs": points_scene("slash", SLASH, [("duration", 0.22), ("radius", 1.2), ("arc", 150), ("width", 0.22)], (0, 0, 0), 3.6),
}
for name, text in files.items():
    (out / name).write_text(text)
    print("wrote", out / name)
