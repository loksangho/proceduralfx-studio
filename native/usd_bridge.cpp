// Small C ABI over OpenUSD for ProceduralFX Studio's STAGE context.
//
// pfx_usd_load opens any USD file (usd/usda/usdc/usdz, with full composition)
// and describes its composed prims as line-based text that the Rust side
// parses (see src/usd.rs, parse_bridge_dump):
//
//   prim <path> <type> <active 0|1> <visible 0|1>
//   xform 16 numbers           local transform, row-major, USD row-vector convention
//   binding <material path>    resolved material binding (inherited bindings included)
//   f <name> <value>           float attribute
//   v <name> <x> <y> <z>       float3 attribute
//   s <name> <token>           token attribute (no spaces)
//   points <n> x y z ...
//   counts <n> ...             faceVertexCounts
//   indices <n> ...            faceVertexIndices
//   color <interpolation> <n> r g b ...   flattened primvars:displayColor
//   lefthanded                 orientation = leftHanded
//
// pfx_usd_export writes USDA text to any USD format chosen by the file extension,
// packaging .usdz through a temporary .usdc.

#include <pxr/pxr.h>
#include <pxr/base/gf/matrix4d.h>
#include <pxr/base/tf/token.h>
#include <pxr/usd/sdf/layer.h>
#include <pxr/usd/usd/primRange.h>
#include <pxr/usd/usd/stage.h>
#include <pxr/usd/usdGeom/camera.h>
#include <pxr/usd/usdGeom/capsule.h>
#include <pxr/usd/usdGeom/cone.h>
#include <pxr/usd/usdGeom/cube.h>
#include <pxr/usd/usdGeom/cylinder.h>
#include <pxr/usd/usdGeom/gprim.h>
#include <pxr/usd/usdGeom/imageable.h>
#include <pxr/usd/usdGeom/mesh.h>
#include <pxr/usd/usdGeom/metrics.h>
#include <pxr/usd/usdGeom/primvarsAPI.h>
#include <pxr/usd/usdGeom/sphere.h>
#include <pxr/usd/usdGeom/xformable.h>
#include <pxr/usd/usdLux/lightAPI.h>
#include <pxr/usd/usdShade/material.h>
#include <pxr/usd/usdShade/materialBindingAPI.h>
#include <pxr/usd/usdShade/shader.h>
#include <pxr/usd/usdUtils/dependencies.h>

#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <map>
#include <mutex>
#include <sstream>
#include <string>

PXR_NAMESPACE_USING_DIRECTIVE

namespace {

char *copy_string(const std::string &text) {
    char *out = static_cast<char *>(std::malloc(text.size() + 1));
    std::memcpy(out, text.c_str(), text.size() + 1);
    return out;
}

void set_error(char **error, const std::string &message) {
    if (error) {
        *error = copy_string(message);
    }
}

// Stages stay open between cooks so animated files are cheap to sample per frame.
std::mutex stage_lock;
std::map<std::string, UsdStageRefPtr> &open_stages() {
    static std::map<std::string, UsdStageRefPtr> stages;
    return stages;
}

template <typename T>
bool get_attr(const UsdAttribute &attr, UsdTimeCode time, T *value) {
    return attr && attr.Get(value, time);
}

void write_float(std::ostringstream &out, const char *name, const UsdAttribute &attr, UsdTimeCode time) {
    float f = 0.0f;
    double d = 0.0;
    if (get_attr(attr, time, &f)) {
        out << "f " << name << ' ' << f << '\n';
    } else if (get_attr(attr, time, &d)) {
        out << "f " << name << ' ' << d << '\n';
    }
}

void write_vec3(std::ostringstream &out, const char *name, const UsdAttribute &attr, UsdTimeCode time) {
    GfVec3f v;
    if (get_attr(attr, time, &v)) {
        out << "v " << name << ' ' << v[0] << ' ' << v[1] << ' ' << v[2] << '\n';
    }
}

void write_token(std::ostringstream &out, const char *name, const UsdAttribute &attr, UsdTimeCode time) {
    TfToken token;
    if (get_attr(attr, time, &token)) {
        out << "s " << name << ' ' << token.GetString() << '\n';
    }
}

void describe_preview_surface(std::ostringstream &out, const UsdShadeMaterial &material, UsdTimeCode time) {
    UsdShadeShader surface = material.ComputeSurfaceSource();
    if (!surface) {
        return;
    }
    TfToken id;
    surface.GetShaderId(&id);
    out << "s shader " << (id.IsEmpty() ? std::string("unknown") : id.GetString()) << '\n';
    for (const char *name : {"diffuseColor", "emissiveColor"}) {
        if (UsdShadeInput input = surface.GetInput(TfToken(name))) {
            GfVec3f v;
            if (input.Get(&v, time)) {
                out << "v " << name << ' ' << v[0] << ' ' << v[1] << ' ' << v[2] << '\n';
            }
        }
    }
    for (const char *name : {"roughness", "metallic", "opacity", "ior"}) {
        if (UsdShadeInput input = surface.GetInput(TfToken(name))) {
            float f = 0.0f;
            if (input.Get(&f, time)) {
                out << "f " << name << ' ' << f << '\n';
            }
        }
    }
}

void describe_prim(std::ostringstream &out, const UsdPrim &prim, UsdTimeCode time) {
    std::string type = prim.GetTypeName().GetString();
    if (type.empty()) {
        type = "Scope";
    }
    UsdGeomImageable imageable(prim);
    bool visible = !imageable || imageable.ComputeVisibility(time) != UsdGeomTokens->invisible;
    out << "prim " << prim.GetPath().GetString() << ' ' << type << ' ' << (prim.IsActive() ? 1 : 0) << ' '
        << (visible ? 1 : 0) << '\n';

    if (UsdGeomXformable xformable{prim}) {
        GfMatrix4d m(1.0);
        bool resets = false;
        xformable.GetLocalTransformation(&m, &resets, time);
        out << "xform";
        for (int r = 0; r < 4; ++r) {
            for (int c = 0; c < 4; ++c) {
                out << ' ' << m[r][c];
            }
        }
        out << '\n';
    }

    if (prim.IsA<UsdGeomGprim>() || prim.IsA<UsdGeomXformable>()) {
        UsdShadeMaterial bound = UsdShadeMaterialBindingAPI(prim).ComputeBoundMaterial();
        if (bound) {
            out << "binding " << bound.GetPath().GetString() << '\n';
        }
    }

    if (UsdGeomGprim gprim{prim}) {
        UsdGeomPrimvar color = UsdGeomPrimvarsAPI(prim).GetPrimvar(TfToken("displayColor"));
        VtArray<GfVec3f> colors;
        if (color && color.ComputeFlattened(&colors, time) && !colors.empty()) {
            out << "color " << color.GetInterpolation().GetString() << ' ' << colors.size();
            for (const GfVec3f &c : colors) {
                out << ' ' << c[0] << ' ' << c[1] << ' ' << c[2];
            }
            out << '\n';
        }
        TfToken orientation;
        if (gprim.GetOrientationAttr().Get(&orientation, time) && orientation == UsdGeomTokens->leftHanded) {
            out << "lefthanded\n";
        }
    }

    if (UsdGeomMesh mesh{prim}) {
        VtArray<GfVec3f> points;
        VtArray<int> counts, indices;
        mesh.GetPointsAttr().Get(&points, time);
        mesh.GetFaceVertexCountsAttr().Get(&counts, time);
        mesh.GetFaceVertexIndicesAttr().Get(&indices, time);
        out << "points " << points.size();
        for (const GfVec3f &p : points) {
            out << ' ' << p[0] << ' ' << p[1] << ' ' << p[2];
        }
        out << "\ncounts " << counts.size();
        for (int c : counts) {
            out << ' ' << c;
        }
        out << "\nindices " << indices.size();
        for (int i : indices) {
            out << ' ' << i;
        }
        out << '\n';
    } else if (UsdGeomSphere sphere{prim}) {
        write_float(out, "radius", sphere.GetRadiusAttr(), time);
    } else if (UsdGeomCube cube{prim}) {
        write_float(out, "size", cube.GetSizeAttr(), time);
    } else if (UsdGeomCylinder cylinder{prim}) {
        write_float(out, "radius", cylinder.GetRadiusAttr(), time);
        write_float(out, "height", cylinder.GetHeightAttr(), time);
        write_token(out, "axis", cylinder.GetAxisAttr(), time);
    } else if (UsdGeomCone cone{prim}) {
        write_float(out, "radius", cone.GetRadiusAttr(), time);
        write_float(out, "height", cone.GetHeightAttr(), time);
        write_token(out, "axis", cone.GetAxisAttr(), time);
    } else if (UsdGeomCapsule capsule{prim}) {
        write_float(out, "radius", capsule.GetRadiusAttr(), time);
        write_float(out, "height", capsule.GetHeightAttr(), time);
        write_token(out, "axis", capsule.GetAxisAttr(), time);
    } else if (UsdGeomCamera camera{prim}) {
        write_float(out, "focalLength", camera.GetFocalLengthAttr(), time);
        write_float(out, "horizontalAperture", camera.GetHorizontalApertureAttr(), time);
        write_float(out, "verticalAperture", camera.GetVerticalApertureAttr(), time);
    } else if (UsdShadeMaterial material{prim}) {
        describe_preview_surface(out, material, time);
    }

    if (prim.HasAPI<UsdLuxLightAPI>()) {
        UsdLuxLightAPI light(prim);
        write_float(out, "intensity", light.GetIntensityAttr(), time);
        write_float(out, "exposure", light.GetExposureAttr(), time);
        write_vec3(out, "color", light.GetColorAttr(), time);
        write_float(out, "radius", prim.GetAttribute(TfToken("inputs:radius")), time);
        write_float(out, "angle", prim.GetAttribute(TfToken("inputs:angle")), time);
        write_float(out, "width", prim.GetAttribute(TfToken("inputs:width")), time);
        write_float(out, "height", prim.GetAttribute(TfToken("inputs:height")), time);
    }
}

}  // namespace

extern "C" {

// Returns the prim description for `path` at `seconds` (converted with the
// stage's timeCodesPerSecond), or null with *error set. Free with pfx_usd_free.
char *pfx_usd_load(const char *path, double seconds, int reload, char **error) {
    try {
        std::lock_guard<std::mutex> guard(stage_lock);
        auto &stages = open_stages();
        auto found = stages.find(path);
        if (found != stages.end() && reload) {
            found->second->Reload();
        }
        if (found == stages.end()) {
            UsdStageRefPtr opened = UsdStage::Open(path);
            if (!opened) {
                set_error(error, std::string("could not open USD file ") + path);
                return nullptr;
            }
            found = stages.emplace(path, opened).first;
        }
        UsdStageRefPtr stage = found->second;
        double fps = stage->GetTimeCodesPerSecond();
        UsdTimeCode time = stage->HasAuthoredTimeCodeRange()
                               ? UsdTimeCode(stage->GetStartTimeCode() + seconds * (fps > 0.0 ? fps : 24.0))
                               : UsdTimeCode::Default();
        std::ostringstream out;
        out.precision(9);
        out << "animated " << (stage->HasAuthoredTimeCodeRange() ? 1 : 0) << '\n';
        out << "upaxis " << UsdGeomGetStageUpAxis(stage).GetString() << '\n';
        out << "meters " << UsdGeomGetStageMetersPerUnit(stage) << '\n';
        for (const UsdPrim &prim : stage->Traverse()) {
            describe_prim(out, prim, time);
        }
        return copy_string(out.str());
    } catch (const std::exception &e) {
        set_error(error, e.what());
        return nullptr;
    }
}

// Writes USDA text to `path`; the extension picks the format (.usda, .usdc, .usd, .usdz).
int pfx_usd_export(const char *usda, const char *path, char **error) {
    try {
        SdfLayerRefPtr layer = SdfLayer::CreateAnonymous(".usda");
        if (!layer->ImportFromString(usda)) {
            set_error(error, "USD rejected the generated scene description");
            return 0;
        }
        // u8path: the path is UTF-8 (Windows would read a plain char path in the ANSI code page).
        std::filesystem::path target = std::filesystem::u8path(path);
        if (target.extension() == ".usdz") {
            std::filesystem::path crate = std::filesystem::temp_directory_path() / (target.stem().u8string() + "_pfx.usdc");
            if (!layer->Export(crate.u8string())) {
                set_error(error, "could not write the temporary .usdc for the .usdz package");
                return 0;
            }
            bool packaged = UsdUtilsCreateNewUsdzPackage(SdfAssetPath(crate.u8string()), target.u8string());
            std::filesystem::remove(crate);
            if (!packaged) {
                set_error(error, std::string("could not create ") + path);
                return 0;
            }
            return 1;
        }
        if (!layer->Export(target.u8string())) {
            set_error(error, std::string("could not write ") + path);
            return 0;
        }
        // The file changed on disk, so drop any stage opened from it.
        std::lock_guard<std::mutex> guard(stage_lock);
        open_stages().erase(path);
        return 1;
    } catch (const std::exception &e) {
        set_error(error, e.what());
        return 0;
    }
}

void pfx_usd_free(char *text) {
    std::free(text);
}

}  // extern "C"
