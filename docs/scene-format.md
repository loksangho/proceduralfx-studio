# The .pfs scene file format

ProceduralFX Studio saves scenes as `.pfs` files. A `.pfs` file is plain UTF-8 text,
one record per line, so you can read it, diff it and write it without the app. This page
describes the whole format. [scene-nodes.md](scene-nodes.md) lists every node type and its
parameters.

Older scenes use the `.mhip` extension. Their format is the same.

## Example

```
pfs 2
node 0 40 20 0 obj geometry -1
node 1 40 20 0 obj grid 4 20
node 2 40 100 0 obj noise 2.2 0.2
link 1 2 0
parent 1 0
parent 2 0
xform 0 0 1 0 0 45 0 1 1 1
name 0 726f636b73
display 0 2
channel 2 p 1 0,0.2,2,0,0;2,0.8,2,0,0
next 3
```

This scene is a Geometry object (node 0) holding a grid (node 1) displaced by noise
(node 2). The object is moved up 1 and turned 45° around Y, and it's named "rocks". The
noise amplitude is keyframed from 0.2 at time 0 to 0.8 at 2 seconds.

## Values

- **Numbers** are decimal, as Rust's `f32` formatting writes them (`1`, `0.65`, `-35`, `1e-5`).
  Readers should accept any decimal notation.
- **Text** (names, code, file paths, patterns) is written as hex-encoded UTF-8 bytes, two
  lowercase hex digits per byte, so it never contains spaces or line breaks. `726f636b73` is "rocks".
- **Node ids** are non-negative integers, unique within the file.
- **Booleans** are `0` or `1`.
- Tokens are separated by single spaces.

## Lines

The first line is the version: `pfs 2`. Files from older versions say `mhip 2` (the same
format) or `mhip 1`. After the version, the lines below come in this order. A reader should
ignore any line it doesn't recognise.

### `node <id> <x> <y> <bypass> <context> <type> <parameters…>`

One node.

| Field | Meaning |
| --- | --- |
| `id` | The node's id. |
| `x`, `y` | Its position in the network editor. |
| `bypass` | `1` if the node is bypassed, so its first input passes through unchanged. |
| `context` | The network it lives in: `obj` (objects and geometry), `mat` (materials), `out` (render outputs), `stage` (USD), `img` (compositing) or `tasks` (task graphs). |
| `type` | The node type, from the "Saved as" column of [scene-nodes.md](scene-nodes.md). |
| `parameters` | The node's parameters, in the order [scene-nodes.md](scene-nodes.md) lists them. |

Parameters are numbers. Nodes that also have text fields write those first, as one token:
`t:` followed by each hex-encoded string, joined by `|`. For example,
`t:2f67656f 1` is a text field "/geo" followed by the number 1.

Animated parameters are written with their value at time 0. Their animation is in
`channel` lines.

Some nodes store several types under one saved name, for example `vop`, `cop`, `top` and
`shader_node`. Their first number is the operation, listed as `operation` in
[scene-nodes.md](scene-nodes.md).

Two node types use their own layout:

- **`wrangle`** stores one token, `<domain>:<code>`. Here `domain` is what the code runs
  over: 0 points, 1 vertices, 2 primitives, 3 once (detail). `code` is the VEX in hex.
- **`moonray_render`** stores one token: `mr:` followed by 20 fields separated by `|`:

  | # | Field | # | Field |
  | --- | --- | --- | --- |
  | 0 | executable (hex text) | 10–12 | background colour r, g, b |
  | 1 | extra arguments (hex text) | 13 | render a frame range (bool) |
  | 2 | output path (hex text) | 14 | range start (seconds) |
  | 3 | width (pixels) | 15 | range end (seconds) |
  | 4 | height (pixels) | 16 | range step (seconds) |
  | 5 | pixel samples | 17 | render the USD stage (bool) |
  | 6 | camera: `auto`, `viewport`, or `n<id>` for a camera node | 18 | volume samples |
  | 7 | maximum ray depth | 19 | render through Hydra (bool) |
  | 8 | light samples | | |
  | 9 | BSDF samples | | |

### `link <from> <to> <input> [<output>]`

A wire from output `output` of node `from` to input `input` of node `to`. Inputs and
outputs count from 0. `output` is left out when it's 0.

### `parent <id> <parent>`

Node `id` lives inside node `parent`, for example a SOP inside a Geometry object, or a node
inside a subnetwork.

### `xform <id> <tx> <ty> <tz> <rx> <ry> <rz> <sx> <sy> <sz>`

An object's transform: translate, rotate and scale. Points are scaled, then rotated (in
degrees, around X, then Y, then Z), then translated. This line is left out for objects with no transform, which is translate 0,
rotate 0, scale 1.

### `chan <id> <name> <x> <y> <z>`

A user channel on a node, read in VEX with `ch("name")`. `name` is hex text. Single-value
channels use `x` and ignore `y` and `z`.

### `name <id> <name>`

The name the user gave a node, as hex text. Nodes without a name show a generated label
such as `noise2`.

### `group <id> <type> <pattern>`

The group a node applies to. `type` is 0 to guess from the group name, 1 for points or 2 for
primitives. `pattern` is hex text, for example a group name or `0-9`.

### `display <container> <id>`

Inside the network `container`, node `id` has the display flag, meaning its output is what
the container shows. An old form, `display <id>`, sets it for the top level.

### `channel <id> <kind> <index> <keys>`

Animation on node `id`. `kind` is `p` for the node's parameter number `index` (counting
from 0, in [scene-nodes.md](scene-nodes.md) order), or `x` for its transform. For `x`,
indices 0–8 are tx, ty, tz, rx, ry, rz, sx, sy, sz.

`keys` is a list separated by `;`. Each keyframe is
`<time>,<value>,<interpolation>,<in slope>,<out slope>`, with time in seconds. Interpolation
is 0 constant, 1 linear, 2 smooth (automatic tangents) or 3 Bézier (the key's own slopes).
An expression comes first, as `expr:` followed by the expression in hex. When there is an
expression, the keyframes are ignored.

### `next <id>`

The id the app gives the next node it creates. It's always higher than every id in the
file.

## Files a scene points to

Scenes refer to external files by path, for example imported meshes, textures, USD
layers and caches. Those files are in their own standard formats, such as OBJ, PNG, EXR, USD
and VDB. They aren't stored inside the `.pfs`.

## Compatibility

New versions of the app keep reading older `.pfs` and `.mhip` files. When a node type gains
a parameter, it's added at the end of its list. A parameter missing from the end of a
`node` line reads as 0, and readers should do the same. If the format ever changes in a way older readers can't follow, the version line
changes, and this page will describe both versions.
