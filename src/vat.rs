// Vertex animation textures (VAT): a deforming mesh baked for game engines. Each frame's
// point positions are stored as offsets from the first frame's, one texel per point, in a
// half-float EXR; a second EXR holds the normals. The first frame is exported as an OBJ
// whose UVs say which texels are each point's. A small shader in the engine reads the
// offsets for the current frame and moves the points on the GPU, so a cloth, a wobbling
// creature or a twisting shape plays back for the cost of one texture read per vertex.
//
// This is the "soft body" kind of VAT: the point count and triangles must stay the same
// for the whole range (deformers, cloth, procedural motion).
//
// Layout: points run left to right, `width` to a row, so a frame takes `rows_per_frame`
// rows; frame f's rows start at f * rows_per_frame. Row 0 is the top of the image. Point
// p's UV is ((p + 0.5) / point_count, 0.5).
//
// Space: the OBJ's (right-handed, like the other exports: z mirrored), so offsets and
// normals are mirrored the same way.

use crate::{Mesh, Vec3};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// GPUs commonly allow textures up to this size on a side; most engines import up to it.
pub const MAX_SIDE: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub points: usize,
    pub frames: usize,
    pub width: usize,
    pub rows_per_frame: usize,
}

impl Layout {
    pub fn new(points: usize, frames: usize) -> Result<Layout, String> {
        if points == 0 || frames == 0 {
            return Err("there's nothing to bake: the geometry has no points".into());
        }
        // As wide as needed, up to the limit; a frame wraps onto more rows past it.
        let width = points.min(MAX_SIDE);
        let rows_per_frame = points.div_ceil(width);
        let layout = Layout { points, frames, width, rows_per_frame };
        if layout.height() > MAX_SIDE {
            return Err(format!(
                "{points} points over {frames} frames needs a {}×{} texture, over the {MAX_SIDE}-pixel limit: bake fewer frames (or every other frame), or use fewer points",
                layout.width,
                layout.height()
            ));
        }
        Ok(layout)
    }

    pub fn height(&self) -> usize {
        self.frames * self.rows_per_frame
    }

    // The texel holding point `p` at frame `f`, from the top left.
    pub fn texel(&self, p: usize, f: usize) -> (usize, usize) {
        (p % self.width, f * self.rows_per_frame + p / self.width)
    }
}

// What gets baked: the first frame's mesh, and every frame's positions and normals.
#[derive(Debug)]
pub struct Bake {
    pub rest: Mesh,
    pub positions: Vec<Vec<Vec3>>,
    pub normals: Vec<Vec<Vec3>>,
}

// Checks the frames can be baked (same points, same triangles) and takes what's needed.
pub fn bake(frames: &[Mesh]) -> Result<Bake, String> {
    let rest = frames.first().ok_or("there are no frames to bake")?.clone();
    if rest.indices.is_empty() {
        return Err("the geometry has no polygons: vertex animation textures move a mesh's points".into());
    }
    for (f, mesh) in frames.iter().enumerate().skip(1) {
        if mesh.vertices.len() != rest.vertices.len() || mesh.indices != rest.indices {
            return Err(format!(
                "the geometry changes at frame {} ({} points, then {}): this bake needs the same points and polygons all the way through, as deformers and cloth keep them. Particles and fluids change theirs",
                f + 1,
                rest.vertices.len(),
                mesh.vertices.len()
            ));
        }
    }
    Ok(Bake { positions: frames.iter().map(|m| m.vertices.clone()).collect(), normals: frames.iter().map(|m| m.compute_normals()).collect(), rest })
}

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

// The first frame as OBJ: positions, each point's lookup UV and normal, and triangles
// wound for the mirrored (right-handed) space.
pub fn rest_obj(bake: &Bake) -> String {
    let n = bake.rest.vertices.len();
    let mut out = String::from("# ProceduralFX Studio vertex animation: the first frame. UVs say which texels are each point's.\n");
    for v in &bake.rest.vertices {
        let p = mirror(*v);
        let _ = writeln!(out, "v {} {} {}", p.x, p.y, p.z);
    }
    for p in 0..n {
        let _ = writeln!(out, "vt {} 0.5", (p as f64 + 0.5) / n as f64);
    }
    for v in &bake.normals[0] {
        let m = mirror(*v);
        let _ = writeln!(out, "vn {} {} {}", m.x, m.y, m.z);
    }
    for [a, b, c] in &bake.rest.indices {
        // Mirroring flips the winding: swap two corners back.
        let _ = writeln!(out, "f {0}/{0}/{0} {2}/{2}/{2} {1}/{1}/{1}", a + 1, b + 1, c + 1);
    }
    out
}

// Every frame's offsets from the first frame (or normals), as texel colours: index
// (x + y * width) * 3.
pub fn texels(bake: &Bake, layout: &Layout, normals: bool) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; layout.width * layout.height()];
    for f in 0..layout.frames {
        for p in 0..layout.points {
            let value = if normals { mirror(bake.normals[f][p]) } else { mirror(bake.positions[f][p]).sub(mirror(bake.rest.vertices[p])) };
            let (x, y) = layout.texel(p, f);
            out[x + y * layout.width] = [value.x, value.y, value.z];
        }
    }
    out
}

fn write_exr(path: &Path, layout: &Layout, data: &[[f32; 3]]) -> Result<(), String> {
    use exr::prelude::*;
    let width = layout.width;
    write_rgba_file(path, width, layout.height(), |x, y| {
        let [r, g, b] = data[x + y * width];
        (f16::from_f32(r), f16::from_f32(g), f16::from_f32(b), f16::ONE)
    })
    .map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

// The files a bake writes beside `stem` (name.obj, name_offsets.exr...).
pub struct Files {
    pub obj: PathBuf,
    pub offsets: PathBuf,
    pub normals: PathBuf,
    pub json: PathBuf,
}

impl Files {
    pub fn new(stem: &Path) -> Files {
        let with = |suffix: &str| {
            let name = stem.file_name().map_or_else(|| "vat".into(), |n| n.to_string_lossy().into_owned());
            stem.with_file_name(format!("{name}{suffix}"))
        };
        Files { obj: with(".obj"), offsets: with("_offsets.exr"), normals: with("_normals.exr"), json: with(".vat.json") }
    }

    pub fn name(&self) -> String {
        self.obj.file_stem().map_or_else(|| "vat".into(), |s| s.to_string_lossy().into_owned())
    }
}

// Writes the mesh, the two textures and the JSON describing them. Returns the layout.
pub fn write(bake: &Bake, stem: &Path, fps: f32) -> Result<(Files, Layout), String> {
    let layout = Layout::new(bake.rest.vertices.len(), bake.positions.len())?;
    let files = Files::new(stem);
    if let Some(dir) = files.obj.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&files.obj, rest_obj(bake)).map_err(|e| e.to_string())?;
    write_exr(&files.offsets, &layout, &texels(bake, &layout, false))?;
    write_exr(&files.normals, &layout, &texels(bake, &layout, true))?;
    let name = |p: &Path| p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let json = serde_json::json!({
        "format": "procedural-fx-vat",
        "version": 1,
        "mode": "soft",
        "mesh": name(&files.obj),
        "offsets": name(&files.offsets),
        "normals": name(&files.normals),
        "point_count": layout.points,
        "frame_count": layout.frames,
        "fps": fps,
        "texture_size": [layout.width, layout.height()],
        "width": layout.width,
        "rows_per_frame": layout.rows_per_frame,
        "uv": "point p's UV is ((p + 0.5) / point_count, 0.5); its texel at frame f is (p % width, f * rows_per_frame + p / width), from the top left",
        "space": "right-handed, y up (the OBJ's): offsets are added to the OBJ's positions",
    });
    std::fs::write(&files.json, serde_json::to_string_pretty(&json).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    Ok((files, layout))
}

// Cooks `object` (a geometry object, with its transform) at each time and bakes it.
// Simulations step as they cook. `progress` hears frames done and the total.
pub fn bake_object(graph: &crate::Graph, object: crate::NodeId, times: &[f32], progress: &mut dyn FnMut(usize, usize)) -> Result<Bake, String> {
    let mut frames = Vec::with_capacity(times.len());
    for (i, &t) in times.iter().enumerate() {
        progress(i, times.len());
        frames.push(graph.cook(object, t, &mut std::collections::HashMap::new()));
    }
    progress(times.len(), times.len());
    bake(&frames)
}

// ---------------------------------------------------------------------------
// Godot 4
// ---------------------------------------------------------------------------

// A spatial shader playing the bake: put it (or name_vat_material.tres) on a MeshInstance3D
// showing name.obj. TIME drives it; set `frame` from a script instead with `use_time` off.
pub fn godot_shader(layout: &Layout, fps: f32) -> String {
    format!(
        "// ProceduralFX Studio vertex animation: moves the mesh's points from the offsets texture.
shader_type spatial;

uniform sampler2D offsets : filter_nearest, repeat_disable;
uniform sampler2D normals : filter_nearest, repeat_disable;
uniform int point_count = {points};
uniform int frame_count = {frames};
uniform int width = {width};
uniform int rows_per_frame = {rows};
uniform float fps = {fps:.3};
uniform bool use_time = true;
uniform bool looping = true;
// The frame to show when use_time is off (from 0; fractions blend to the next frame).
uniform float frame = 0.0;
uniform vec4 albedo : source_color = vec4(1.0);
uniform float roughness : hint_range(0.0, 1.0) = 0.6;

vec3 fetch(sampler2D tex, int point, int f) {{
	return texelFetch(tex, ivec2(point % width, f * rows_per_frame + point / width), 0).rgb;
}}

void vertex() {{
	int point = clamp(int(UV.x * float(point_count)), 0, point_count - 1);
	float at = use_time ? TIME * fps : frame;
	at = looping ? mod(at, float(frame_count)) : clamp(at, 0.0, float(frame_count - 1));
	int f0 = int(floor(at));
	int f1 = looping ? (f0 + 1) % frame_count : min(f0 + 1, frame_count - 1);
	float blend = fract(at);
	VERTEX += mix(fetch(offsets, point, f0), fetch(offsets, point, f1), blend);
	NORMAL = normalize(mix(fetch(normals, point, f0), fetch(normals, point, f1), blend));
}}

void fragment() {{
	ALBEDO = albedo.rgb;
	ROUGHNESS = roughness;
}}
",
        points = layout.points,
        frames = layout.frames,
        width = layout.width,
        rows = layout.rows_per_frame,
    )
}

// A ShaderMaterial using the shader and both textures, found beside it.
pub fn godot_material(files: &Files) -> String {
    let name = files.name();
    format!(
        "[gd_resource type=\"ShaderMaterial\" load_steps=4 format=3]

[ext_resource type=\"Shader\" path=\"{name}_vat.gdshader\" id=\"1_shader\"]
[ext_resource type=\"Texture2D\" path=\"{name}_offsets.exr\" id=\"2_offsets\"]
[ext_resource type=\"Texture2D\" path=\"{name}_normals.exr\" id=\"3_normals\"]

[resource]
shader = ExtResource(\"1_shader\")
shader_parameter/offsets = ExtResource(\"2_offsets\")
shader_parameter/normals = ExtResource(\"3_normals\")
"
    )
}

// Import settings for a data texture: lossless (Godot would otherwise compress it once
// it's used in 3D, ruining the numbers), no mipmaps.
pub fn godot_texture_import() -> String {
    "[remap]

importer=\"texture\"
type=\"CompressedTexture2D\"

[params]

compress/mode=0
compress/high_quality=false
compress/lossy_quality=0.7
compress/hdr_compression=0
compress/normal_map=2
compress/channel_pack=0
mipmaps/generate=false
mipmaps/limit=-1
roughness/mode=0
process/fix_alpha_border=false
process/premult_alpha=false
process/normal_map_invert_y=false
process/hdr_as_srgb=false
process/hdr_clamp_exposure=false
process/size_limit=0
detect_3d/compress_to=0
"
    .to_owned()
}

pub fn godot_paths(files: &Files) -> [PathBuf; 4] {
    let name = files.name();
    let beside = |file: String| files.obj.with_file_name(file);
    [beside(format!("{name}_vat.gdshader")), beside(format!("{name}_vat_material.tres")), beside(format!("{name}_offsets.exr.import")), beside(format!("{name}_normals.exr.import"))]
}

pub fn write_godot(files: &Files, layout: &Layout, fps: f32) -> Result<(), String> {
    let [shader, material, offsets_import, normals_import] = godot_paths(files);
    for (path, text) in [(shader, godot_shader(layout, fps)), (material, godot_material(files)), (offsets_import, godot_texture_import()), (normals_import, godot_texture_import())] {
        std::fs::write(&path, text).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unity (URP)
// ---------------------------------------------------------------------------

// A 32-hex-digit Unity guid: the one already in `meta`'s file if there is one (so
// re-exports keep materials linked), else a new one.
fn unity_guid(meta: &Path, seed: &str) -> String {
    use sha2::{Digest, Sha256};
    if let Some(guid) = std::fs::read_to_string(meta).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("guid: ").map(|g| g.trim().to_owned()))) {
        if guid.len() == 32 && guid.chars().all(|c| c.is_ascii_hexdigit()) {
            return guid;
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    Sha256::digest(format!("{seed}:{}:{stamp}", meta.display()).as_bytes())[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn meta_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".meta");
    PathBuf::from(name)
}

pub fn unity_paths(files: &Files) -> [PathBuf; 2] {
    let name = files.name();
    [files.obj.with_file_name(format!("{name}_vat.shader")), files.obj.with_file_name(format!("{name}_vat.mat"))]
}

// A URP shader playing the bake. Unity's OBJ import mirrors x (to its left-handed space),
// and its textures' rows run from the bottom, so both are undone here. Needs the Linear
// colour space (URP's default): in Gamma, Unity changes the textures' values, so the
// shader shows magenta instead of wrong motion.
pub fn unity_shader(name: &str, layout: &Layout, fps: f32) -> String {
    format!(
        "// ProceduralFX Studio vertex animation (URP): moves the mesh's points from the offsets texture.
Shader \"ProceduralFX/Vertex Animation/{name}\"
{{
    Properties
    {{
        [NoScaleOffset] _Offsets(\"Offsets\", 2D) = \"black\" {{}}
        [NoScaleOffset] _Normals(\"Normals\", 2D) = \"black\" {{}}
        _BaseColor(\"Color\", Color) = (1, 1, 1, 1)
        _Fps(\"Frames per second\", Float) = {fps:.3}
        [Toggle] _UseTime(\"Play by itself\", Float) = 1
        [Toggle] _Loop(\"Loop\", Float) = 1
        _Frame(\"Frame (when not playing by itself)\", Float) = 0
        [HideInInspector] _PointCount(\"Point count\", Float) = {points}
        [HideInInspector] _FrameCount(\"Frame count\", Float) = {frames}
        [HideInInspector] _Width(\"Texture width\", Float) = {width}
        [HideInInspector] _RowsPerFrame(\"Rows per frame\", Float) = {rows}
        [HideInInspector] _TextureHeight(\"Texture height\", Float) = {height}
    }}
    SubShader
    {{
        Tags {{ \"RenderType\" = \"Opaque\" \"RenderPipeline\" = \"UniversalPipeline\" \"Queue\" = \"Geometry\" }}
        Pass
        {{
            Name \"ForwardLit\"
            Tags {{ \"LightMode\" = \"UniversalForward\" }}
            HLSLPROGRAM
            #pragma vertex vert
            #pragma fragment frag
            #pragma target 3.5
            #include \"Packages/com.unity.render-pipelines.universal/ShaderLibrary/Core.hlsl\"
            #include \"Packages/com.unity.render-pipelines.universal/ShaderLibrary/Lighting.hlsl\"

            TEXTURE2D(_Offsets);
            TEXTURE2D(_Normals);
            CBUFFER_START(UnityPerMaterial)
                float4 _BaseColor;
                float _Fps, _UseTime, _Loop, _Frame, _PointCount, _FrameCount, _Width, _RowsPerFrame, _TextureHeight;
            CBUFFER_END

            struct Attributes {{ float4 positionOS : POSITION; float3 normalOS : NORMAL; float2 uv : TEXCOORD0; }};
            struct Varyings {{ float4 positionCS : SV_POSITION; float3 normalWS : TEXCOORD0; }};

            // Point p's texel at frame f, counting rows from the bottom as Unity does.
            int2 Texel(int p, int f)
            {{
                int width = (int)_Width;
                return int2(p % width, (int)_TextureHeight - 1 - (f * (int)_RowsPerFrame + p / width));
            }}

            Varyings vert(Attributes v)
            {{
                int count = (int)_FrameCount;
                int p = clamp((int)(v.uv.x * _PointCount), 0, (int)_PointCount - 1);
                float at = _UseTime > 0.5 ? _Time.y * _Fps : _Frame;
                at = _Loop > 0.5 ? fmod(at, _FrameCount) : clamp(at, 0.0, _FrameCount - 1.0);
                int f0 = (int)floor(at);
                int f1 = _Loop > 0.5 ? (f0 + 1) % count : min(f0 + 1, count - 1);
                float blend = frac(at);
                float3 offset = lerp(LOAD_TEXTURE2D(_Offsets, Texel(p, f0)).rgb, LOAD_TEXTURE2D(_Offsets, Texel(p, f1)).rgb, blend);
                float3 normal = normalize(lerp(LOAD_TEXTURE2D(_Normals, Texel(p, f0)).rgb, LOAD_TEXTURE2D(_Normals, Texel(p, f1)).rgb, blend));
                // Unity's OBJ import mirrors x.
                offset.x = -offset.x;
                normal.x = -normal.x;
                Varyings o;
                o.positionCS = TransformObjectToHClip(v.positionOS.xyz + offset);
                o.normalWS = TransformObjectToWorldNormal(normal);
                return o;
            }}

            half4 frag(Varyings i) : SV_Target
            {{
            #ifdef UNITY_COLORSPACE_GAMMA
                // In Gamma colour space Unity bends the textures' numbers, so the motion is
                // wrong: show that plainly. Project Settings > Player > Color Space: Linear.
                return half4(1.0, 0.0, 1.0, 1.0);
            #endif
                float3 n = normalize(i.normalWS);
                Light light = GetMainLight();
                float3 lit = light.color * saturate(dot(n, light.direction)) + SampleSH(n);
                return half4(_BaseColor.rgb * lit, 1.0);
            }}
            ENDHLSL
        }}
    }}
}}
",
        points = layout.points,
        frames = layout.frames,
        width = layout.width,
        rows = layout.rows_per_frame,
        height = layout.height(),
    )
}

// Import settings for a data texture: linear, no mipmaps, nearest filtering, clamped,
// uncompressed, never resized; readable, for scripts.
fn unity_texture_meta(guid: &str, largest: usize) -> String {
    let size = largest.next_power_of_two().clamp(32, 16384);
    format!(
        "fileFormatVersion: 2
guid: {guid}
TextureImporter:
  internalIDToNameTable: []
  externalObjects: {{}}
  serializedVersion: 13
  mipmaps:
    mipMapMode: 0
    enableMipMap: 0
    sRGBTexture: 0
    linearTexture: 0
    fadeOut: 0
    borderMipMap: 0
    mipMapsPreserveCoverage: 0
    alphaTestReferenceValue: 0.5
    mipMapFadeDistanceStart: 1
    mipMapFadeDistanceEnd: 3
  bumpmap:
    convertToNormalMap: 0
    externalNormalMap: 0
    heightScale: 0.25
    normalMapFilter: 0
    flipGreenChannel: 0
  isReadable: 1
  streamingMipmaps: 0
  streamingMipmapsPriority: 0
  vTOnly: 0
  ignoreMipmapLimit: 1
  grayScaleToAlpha: 0
  generateCubemap: 6
  cubemapConvolution: 0
  seamlessCubemap: 0
  textureFormat: 1
  maxTextureSize: {size}
  textureSettings:
    serializedVersion: 2
    filterMode: 0
    aniso: 1
    mipBias: 0
    wrapU: 1
    wrapV: 1
    wrapW: 1
  nPOTScale: 0
  lightmap: 0
  compressionQuality: 50
  spriteMode: 0
  spriteExtrude: 1
  spriteMeshType: 1
  alignment: 0
  spritePivot: {{x: 0.5, y: 0.5}}
  spritePixelsToUnits: 100
  spriteBorder: {{x: 0, y: 0, z: 0, w: 0}}
  spriteGenerateFallbackPhysicsShape: 1
  alphaUsage: 0
  alphaIsTransparency: 0
  spriteTessellationDetail: -1
  textureType: 0
  textureShape: 1
  singleChannelComponent: 0
  flipbookRows: 1
  flipbookColumns: 1
  maxTextureSizeSet: 0
  compressionQualitySet: 0
  textureFormatSet: 0
  ignorePngGamma: 0
  applyGammaDecoding: 0
  swizzle: 50462976
  cookieLightType: 0
  platformSettings:
  - serializedVersion: 4
    buildTarget: DefaultTexturePlatform
    maxTextureSize: {size}
    resizeAlgorithm: 0
    textureFormat: -1
    textureCompression: 0
    compressionQuality: 50
    crunchedCompression: 0
    allowsAlphaSplitting: 0
    overridden: 0
    ignorePlatformSupport: 0
    androidETC2FallbackOverride: 0
    forceMaximumCompressionQuality_BC6H_BC7: 0
  spriteSheet:
    serializedVersion: 2
    sprites: []
    outline: []
    customData: 
    physicsShape: []
    bones: []
    spriteID: 
    internalID: 0
    vertices: []
    indices: 
    edges: []
    weights: []
    secondaryTextures: []
    spriteCustomMetadata:
      entries: []
    nameFileIdTable: {{}}
  mipmapLimitGroupName: 
  pSDRemoveMatte: 0
  userData: 
  assetBundleName: 
  assetBundleVariant: 
"
    )
}

fn unity_material(name: &str, shader: &str, offsets: &str, normals: &str, layout: &Layout, fps: f32) -> String {
    format!(
        "%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!21 &2100000
Material:
  serializedVersion: 8
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_Name: {name}_vat
  m_Shader: {{fileID: 4800000, guid: {shader}, type: 3}}
  m_Parent: {{fileID: 0}}
  m_ModifiedSerializedProperties: 0
  m_ValidKeywords: []
  m_InvalidKeywords: []
  m_LightmapFlags: 4
  m_EnableInstancingVariants: 0
  m_DoubleSidedGI: 0
  m_CustomRenderQueue: -1
  stringTagMap: {{}}
  disabledShaderPasses: []
  m_LockedProperties: 
  m_SavedProperties:
    serializedVersion: 3
    m_TexEnvs:
    - _Normals:
        m_Texture: {{fileID: 2800000, guid: {normals}, type: 3}}
        m_Scale: {{x: 1, y: 1}}
        m_Offset: {{x: 0, y: 0}}
    - _Offsets:
        m_Texture: {{fileID: 2800000, guid: {offsets}, type: 3}}
        m_Scale: {{x: 1, y: 1}}
        m_Offset: {{x: 0, y: 0}}
    m_Ints: []
    m_Floats:
    - _Fps: {fps}
    - _Frame: 0
    - _FrameCount: {frames}
    - _Loop: 1
    - _PointCount: {points}
    - _RowsPerFrame: {rows}
    - _TextureHeight: {height}
    - _UseTime: 1
    - _Width: {width}
    m_Colors:
    - _BaseColor: {{r: 1, g: 1, b: 1, a: 1}}
  m_BuildTextureStacks: []
  m_AllowLocking: 1
",
        points = layout.points,
        frames = layout.frames,
        width = layout.width,
        rows = layout.rows_per_frame,
        height = layout.height(),
    )
}

// The shader, a material using it with both textures, and import settings (.meta files)
// for the textures, shader and material, beside the bake.
pub fn write_unity(files: &Files, layout: &Layout, fps: f32) -> Result<(), String> {
    let name = files.name();
    let [shader, material] = unity_paths(files);
    let guid = |path: &Path| unity_guid(&meta_path(path), &name);
    let (shader_guid, material_guid, offsets_guid, normals_guid) = (guid(&shader), guid(&material), guid(&files.offsets), guid(&files.normals));
    let largest = layout.width.max(layout.height());
    let shader_meta = format!("fileFormatVersion: 2\nguid: {shader_guid}\nShaderImporter:\n  externalObjects: {{}}\n  defaultTextures: []\n  nonModifiableTextures: []\n  preprocessorOverride: 0\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n");
    let material_meta = format!("fileFormatVersion: 2\nguid: {material_guid}\nNativeFormatImporter:\n  externalObjects: {{}}\n  mainObjectFileID: 2100000\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n");
    let writes = [
        (shader.clone(), unity_shader(&name, layout, fps)),
        (meta_path(&shader), shader_meta),
        (material.clone(), unity_material(&name, &shader_guid, &offsets_guid, &normals_guid, layout, fps)),
        (meta_path(&material), material_meta),
        (meta_path(&files.offsets), unity_texture_meta(&offsets_guid, largest)),
        (meta_path(&files.normals), unity_texture_meta(&normals_guid, largest)),
    ];
    for (path, text) in writes {
        std::fs::write(&path, text).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A triangle whose first point rises by 0.1 a frame.
    fn frames(count: usize) -> Vec<Mesh> {
        (0..count)
            .map(|f| Mesh {
                vertices: vec![Vec3::new(0.0, f as f32 * 0.1, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0)],
                indices: vec![[0, 1, 2]],
                ..Mesh::default()
            })
            .collect()
    }

    #[test]
    fn layouts_wrap_wide_frames_onto_more_rows() {
        assert_eq!(Layout::new(3, 10).unwrap(), Layout { points: 3, frames: 10, width: 3, rows_per_frame: 1 });
        let wide = Layout::new(20_000, 48).unwrap();
        assert_eq!((wide.width, wide.rows_per_frame, wide.height()), (8192, 3, 144));
        assert_eq!(wide.texel(8192, 0), (0, 1));
        assert_eq!(wide.texel(8193, 2), (1, 7));
        assert!(Layout::new(8192, 9000).unwrap_err().contains("over the 8192-pixel limit"));
        assert!(Layout::new(0, 10).is_err());
    }

    #[test]
    fn bakes_need_the_same_points_throughout() {
        assert!(bake(&frames(4)).is_ok());
        let mut changing = frames(4);
        changing[2].vertices.push(Vec3::zero());
        assert!(bake(&changing).unwrap_err().contains("changes at frame 3"));
        assert!(bake(&[Mesh { vertices: vec![Vec3::zero()], ..Mesh::default() }]).unwrap_err().contains("no polygons"));
    }

    #[test]
    fn offsets_are_from_the_first_frame_in_the_objs_space() {
        let mut moving = frames(3);
        // The third point also moves along z, which the OBJ mirrors.
        moving[2].vertices[2] = Vec3::new(0.0, 0.0, 1.5);
        let b = bake(&moving).unwrap();
        let layout = Layout::new(3, 3).unwrap();
        let t = texels(&b, &layout, false);
        assert_eq!(t[0], [0.0, 0.0, 0.0], "frame 1 is the rest pose");
        assert!((t[layout.texel(0, 2).0 + layout.texel(0, 2).1 * 3][1] - 0.2).abs() < 1e-6, "point 0 rose 0.2 by frame 3");
        assert_eq!(t[2 + 2 * 3], [0.0, 0.0, -0.5], "z is mirrored");
        let obj = rest_obj(&b);
        assert!(obj.contains("v 0 0 -1\n"), "positions are mirrored like the other exports");
        assert!(obj.contains("vt 0.5 0.5\n"), "the middle point's UV is the middle of its row");
        assert!(obj.contains("f 1/1/1 3/3/3 2/2/2\n"));
    }

    // An animated scene (moving noise on a grid) baked: the rest pose plus each frame's
    // offsets is that frame's shape, as the shaders rebuild it.
    #[test]
    fn baked_offsets_rebuild_every_frame() {
        let graph = crate::Graph::deserialize("pfs 2\nnode 0 40 20 0 obj geometry -1\nnode 1 40 20 0 obj grid 4 8\nnode 2 40 100 0 obj noise 2.2 0.65\nparent 1 0\nparent 2 0\nlink 1 2 0\ndisplay 0 2\nnext 3\n");
        let times: Vec<f32> = (0..6).map(|f| f as f32 / 24.0).collect();
        let mut seen = Vec::new();
        let b = bake_object(&graph, 0, &times, &mut |done, total| seen.push((done, total))).unwrap();
        assert_eq!(seen.last(), Some(&(6, 6)));
        let layout = Layout::new(b.rest.vertices.len(), times.len()).unwrap();
        let offsets = texels(&b, &layout, false);
        let mut moved = 0.0f32;
        for (f, &t) in times.iter().enumerate() {
            let truth = graph.cook(0, t, &mut std::collections::HashMap::new());
            for p in 0..layout.points {
                let (x, y) = layout.texel(p, f);
                let [dx, dy, dz] = offsets[x + y * layout.width];
                let rebuilt = mirror(b.rest.vertices[p]).add(Vec3::new(dx, dy, dz));
                assert!(rebuilt.sub(mirror(truth.vertices[p])).length() < 1e-5, "point {p} at frame {f}");
                moved = moved.max(Vec3::new(dx, dy, dz).length());
            }
        }
        assert!(moved > 0.01, "the noise moves the grid, so this tests something");
    }

    #[test]
    fn engine_files_carry_the_layout_and_keep_unity_ids() {
        let dir = std::env::temp_dir().join(format!("pfx_vat_engines_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (files, layout) = write(&bake(&frames(5)).unwrap(), &dir.join("wobble"), 12.0).unwrap();
        write_godot(&files, &layout, 12.0).unwrap();
        write_unity(&files, &layout, 12.0).unwrap();
        let read = |p: &Path| std::fs::read_to_string(p).unwrap();
        let [shader, material, offsets_import, _] = godot_paths(&files);
        let gd = read(&shader);
        assert!(gd.contains("uniform int point_count = 3;") && gd.contains("uniform int frame_count = 5;") && gd.contains("uniform float fps = 12.000;"));
        assert!(read(&material).contains("path=\"wobble_offsets.exr\""));
        assert!(read(&offsets_import).contains("compress/mode=0") && read(&offsets_import).contains("detect_3d/compress_to=0"));
        let [unity_shader_path, unity_material_path] = unity_paths(&files);
        let hlsl = read(&unity_shader_path);
        assert!(hlsl.contains("Shader \"ProceduralFX/Vertex Animation/wobble\"") && hlsl.contains("_TextureHeight(\"Texture height\", Float) = 5"));
        assert!(hlsl.contains("UNITY_COLORSPACE_GAMMA"));
        let guid_of = |p: &Path| read(&meta_path(p)).lines().find_map(|l| l.strip_prefix("guid: ").map(str::to_owned)).unwrap();
        let mat = read(&unity_material_path);
        assert!(mat.contains(&format!("guid: {}, type: 3}}", guid_of(&unity_shader_path))), "the material uses the shader");
        assert!(mat.contains(&guid_of(&files.offsets)) && mat.contains(&guid_of(&files.normals)), "and both textures");
        let texture_meta = read(&meta_path(&files.offsets));
        assert!(texture_meta.contains("sRGBTexture: 0") && texture_meta.contains("filterMode: 0") && texture_meta.contains("nPOTScale: 0") && texture_meta.contains("textureCompression: 0"));
        // Exporting again keeps every id, so scenes using the material stay linked.
        let before = guid_of(&unity_material_path);
        write_unity(&files, &layout, 12.0).unwrap();
        assert_eq!(guid_of(&unity_material_path), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_makes_the_mesh_textures_and_description() {
        let dir = std::env::temp_dir().join(format!("pfx_vat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (files, layout) = write(&bake(&frames(5)).unwrap(), &dir.join("wobble"), 24.0).unwrap();
        assert_eq!(files.name(), "wobble");
        for path in [&files.obj, &files.offsets, &files.normals, &files.json] {
            assert!(path.is_file(), "{}", path.display());
        }
        let image = exr::prelude::read_first_rgba_layer_from_file(
            &files.offsets,
            |size, _| vec![[0.0f32; 3]; size.width() * size.height()],
            |pixels, at, (r, g, b, _): (f32, f32, f32, f32)| pixels[at.x() + at.y() * 3] = [r, g, b],
        )
        .unwrap();
        assert_eq!((image.layer_data.size.width(), image.layer_data.size.height()), (3, 5));
        // Point 0 at frame 5 (row 4) has risen 0.4, to half-float precision.
        assert!((image.layer_data.channel_data.pixels[4 * 3][1] - 0.4).abs() < 1e-3);
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&files.json).unwrap()).unwrap();
        assert_eq!(json["frame_count"], 5);
        assert_eq!(json["rows_per_frame"], layout.rows_per_frame);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
