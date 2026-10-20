// Small C ABI over Alembic (Ogawa) for ProceduralFX Studio's File Import and Export.
//
// pfx_abc_read describes every PolyMesh, SubD and Points object at a time, in
// world space (their Xforms applied), as line-based text that src/abc.rs parses:
//
//   range <start> <end>                archive time range in seconds
//   mesh <name>                        a PolyMesh or SubD
//   points <n> x y z ...
//   counts <n> ...                     face vertex counts
//   indices <n> ...                    face vertex indices (Alembic's clockwise winding)
//   uv <scope> <n> u v ...             scope: point (one per point) or corner (one per face vertex)
//   normals <scope> <n> x y z ...
//   color <scope> <n> r g b ...        an arbGeomParam Cd / color / displayColor; scope may also be face
//   particles <name>                   a Points object
//   points <n> ... / ids <n> ... / widths <n> ...
//
// pfx_abc_write writes the text src/abc.rs produces: frames of one mesh or one
// point cloud, sampled uniformly at `fps`:
//
//   fps <f> kind <mesh|points> name <name>
//   frame <seconds>
//   points <n> ... counts <n> ... indices <n> ... uv <n> ... normals <n> ... color <scope> <n> ...
//   ids <n> ... widths <n> ...

#include <Alembic/AbcCoreFactory/All.h>
#include <Alembic/AbcCoreOgawa/All.h>
#include <Alembic/AbcGeom/All.h>

#include <cstdlib>
#include <cstring>
#include <sstream>
#include <string>
#include <vector>

using namespace Alembic::AbcGeom;

namespace {

char *copy_string(const std::string &text) {
    char *out = static_cast<char *>(std::malloc(text.size() + 1));
    std::memcpy(out, text.c_str(), text.size() + 1);
    return out;
}

const char *scope_name(GeometryScope scope) {
    switch (scope) {
    case kFacevaryingScope:
        return "corner";
    case kUniformScope:
        return "face";
    default:
        return "point";
    }
}

template <typename T> void write_list(std::ostringstream &out, const char *name, const T *values, size_t count, int components) {
    out << name << ' ' << count;
    for (size_t i = 0; i < count; ++i) {
        for (int c = 0; c < components; ++c) {
            out << ' ' << values[i][c];
        }
    }
    out << '\n';
}

// Looks for a colour among the arbitrary geometry parameters.
void write_color(std::ostringstream &out, ICompoundProperty arb, const ISampleSelector &sel) {
    if (!arb.valid()) {
        return;
    }
    for (const char *name : {"Cd", "color", "displayColor"}) {
        const PropertyHeader *header = arb.getPropertyHeader(name);
        if (!header || !IC3fGeomParam::matches(*header)) {
            continue;
        }
        IC3fGeomParam param(arb, name);
        IC3fGeomParam::Sample sample = param.getExpandedValue(sel);
        if (sample.getVals()) {
            out << "color " << scope_name(param.getScope()) << ' ';
            write_list(out, "", sample.getVals()->get(), sample.getVals()->size(), 3);
        }
        return;
    }
}

void write_points(std::ostringstream &out, const P3fArraySamplePtr &positions, const M44d &world) {
    out << "points " << positions->size();
    for (size_t i = 0; i < positions->size(); ++i) {
        V3d p = V3d((*positions)[i]) * world;
        out << ' ' << p.x << ' ' << p.y << ' ' << p.z;
    }
    out << '\n';
}

template <typename Schema> void write_faces(std::ostringstream &out, Schema &schema, const ISampleSelector &sel, const M44d &world) {
    typename Schema::Sample sample;
    schema.get(sample, sel);
    write_points(out, sample.getPositions(), world);
    auto ints = [&](const char *name, const Int32ArraySamplePtr &values) {
        out << name << ' ' << values->size();
        for (size_t i = 0; i < values->size(); ++i) {
            out << ' ' << (*values)[i];
        }
        out << '\n';
    };
    ints("counts", sample.getFaceCounts());
    ints("indices", sample.getFaceIndices());
    IV2fGeomParam uvs = schema.getUVsParam();
    if (uvs.valid()) {
        IV2fGeomParam::Sample uv = uvs.getExpandedValue(sel);
        if (uv.getVals()) {
            out << "uv " << scope_name(uvs.getScope()) << ' ';
            write_list(out, "", uv.getVals()->get(), uv.getVals()->size(), 2);
        }
    }
}

void write_normals(std::ostringstream &out, IN3fGeomParam normals, const ISampleSelector &sel, const M44d &world) {
    if (!normals.valid()) {
        return;
    }
    IN3fGeomParam::Sample sample = normals.getExpandedValue(sel);
    if (!sample.getVals()) {
        return;
    }
    out << "normals " << scope_name(normals.getScope()) << ' ' << sample.getVals()->size();
    for (size_t i = 0; i < sample.getVals()->size(); ++i) {
        V3d n;
        world.multDirMatrix(V3d((*sample.getVals())[i]), n);
        n.normalize();
        out << ' ' << n.x << ' ' << n.y << ' ' << n.z;
    }
    out << '\n';
}

void walk(std::ostringstream &out, IObject parent, const M44d &parent_world, const ISampleSelector &sel) {
    for (size_t i = 0; i < parent.getNumChildren(); ++i) {
        IObject child = parent.getChild(i);
        const ObjectHeader &header = child.getHeader();
        M44d world = parent_world;
        if (IXform::matches(header)) {
            IXform xform(child, kWrapExisting);
            XformSample sample;
            xform.getSchema().get(sample, sel);
            // Row vectors: the child's matrix first, then its parent's.
            world = xform.getSchema().getInheritsXforms(sel) ? sample.getMatrix() * parent_world : sample.getMatrix();
        } else if (IPolyMesh::matches(header)) {
            IPolyMesh mesh(child, kWrapExisting);
            IPolyMeshSchema &schema = mesh.getSchema();
            out << "mesh " << child.getFullName() << '\n';
            write_faces(out, schema, sel, world);
            write_normals(out, schema.getNormalsParam(), sel, world);
            write_color(out, schema.getArbGeomParams(), sel);
        } else if (ISubD::matches(header)) {
            ISubD subd(child, kWrapExisting);
            ISubDSchema &schema = subd.getSchema();
            out << "mesh " << child.getFullName() << '\n';
            write_faces(out, schema, sel, world);
            write_color(out, schema.getArbGeomParams(), sel);
        } else if (IPoints::matches(header)) {
            IPoints points(child, kWrapExisting);
            IPointsSchema &schema = points.getSchema();
            IPointsSchema::Sample sample;
            schema.get(sample, sel);
            out << "particles " << child.getFullName() << '\n';
            write_points(out, sample.getPositions(), world);
            if (sample.getIds()) {
                out << "ids " << sample.getIds()->size();
                for (size_t k = 0; k < sample.getIds()->size(); ++k) {
                    out << ' ' << (*sample.getIds())[k];
                }
                out << '\n';
            }
            IFloatGeomParam widths = schema.getWidthsParam();
            if (widths.valid()) {
                IFloatGeomParam::Sample w = widths.getExpandedValue(sel);
                if (w.getVals()) {
                    out << "widths " << w.getVals()->size();
                    for (size_t k = 0; k < w.getVals()->size(); ++k) {
                        out << ' ' << (*w.getVals())[k];
                    }
                    out << '\n';
                }
            }
            write_color(out, schema.getArbGeomParams(), sel);
        }
        walk(out, child, world, sel);
    }
}

// Whitespace-separated reader for pfx_abc_write's input.
struct Reader {
    std::istringstream in;
    explicit Reader(const char *text) : in(text) {}
    std::string word() {
        std::string w;
        in >> w;
        return w;
    }
    template <typename T> std::vector<T> list() {
        size_t n = 0;
        in >> n;
        std::vector<T> values(n);
        for (auto &v : values) {
            in >> v;
        }
        return values;
    }
};

} // namespace

extern "C" {

char *pfx_abc_read(const char *path, double seconds, char **error) {
    try {
        Alembic::AbcCoreFactory::IFactory factory;
        IArchive archive = factory.getArchive(path);
        if (!archive.valid()) {
            *error = copy_string(std::string("not an Alembic file: ") + path);
            return nullptr;
        }
        std::ostringstream out;
        out.precision(9);
        double start = 0.0, end = 0.0;
        GetArchiveStartAndEndTime(archive, start, end);
        out << "range " << start << ' ' << end << '\n';
        walk(out, archive.getTop(), M44d(), ISampleSelector(seconds));
        return copy_string(out.str());
    } catch (const std::exception &e) {
        *error = copy_string(e.what());
        return nullptr;
    }
}

int pfx_abc_write(const char *path, const char *data, char **error) {
    try {
        Reader r(data);
        double fps = 24.0;
        std::string kind = "mesh", name = "geometry";
        if (r.word() != "fps") {
            throw std::runtime_error("malformed export data");
        }
        r.in >> fps;
        r.word();
        r.in >> kind;
        r.word();
        r.in >> name;
        OArchive archive = CreateArchiveWithInfo(Alembic::AbcCoreOgawa::WriteArchive(), path, "ProceduralFX Studio", "");
        bool first = true;
        double first_time = 0.0;
        OPolyMesh mesh;
        OPoints points;
        OC3fGeomParam color;
        GeometryScope color_scope = kVertexScope;
        std::string word = r.word();
        while (word == "frame") {
            double t = 0.0;
            r.in >> t;
            std::vector<float> p, uv, n, cd, widths;
            std::vector<int32_t> counts, indices;
            std::vector<uint64_t> ids;
            std::string scope = "point";
            for (word = r.word(); !word.empty() && word != "frame"; word = r.word()) {
                if (word == "points") p = r.list<float>();
                else if (word == "counts") counts = r.list<int32_t>();
                else if (word == "indices") indices = r.list<int32_t>();
                else if (word == "uv") uv = r.list<float>();
                else if (word == "normals") n = r.list<float>();
                else if (word == "color") { r.in >> scope; cd = r.list<float>(); }
                else if (word == "ids") ids = r.list<uint64_t>();
                else if (word == "widths") widths = r.list<float>();
                else throw std::runtime_error("unknown export field " + word);
            }
            if (first) {
                first_time = t;
                TimeSamplingPtr sampling(new TimeSampling(1.0 / fps, first_time));
                uint32_t index = archive.addTimeSampling(*sampling);
                color_scope = scope == "corner" ? kFacevaryingScope : scope == "face" ? kUniformScope : kVertexScope;
                if (kind == "points") {
                    points = OPoints(OObject(archive, kTop), name, index);
                    if (!cd.empty()) color = OC3fGeomParam(points.getSchema().getArbGeomParams(), "Cd", false, color_scope, 1, index);
                } else {
                    mesh = OPolyMesh(OObject(archive, kTop), name, index);
                    if (!cd.empty()) color = OC3fGeomParam(mesh.getSchema().getArbGeomParams(), "Cd", false, color_scope, 1, index);
                }
                first = false;
            }
            const V3f *positions = reinterpret_cast<const V3f *>(p.data());
            if (kind == "points") {
                if (ids.empty()) {
                    for (size_t i = 0; i < p.size() / 3; ++i) ids.push_back(i);
                }
                OFloatGeomParam::Sample w(FloatArraySample(widths), kVertexScope);
                OPointsSchema::Sample sample(V3fArraySample(positions, p.size() / 3), UInt64ArraySample(ids), V3fArraySample(), widths.empty() ? OFloatGeomParam::Sample() : w);
                points.getSchema().set(sample);
            } else {
                OV2fGeomParam::Sample uvs(V2fArraySample(reinterpret_cast<const V2f *>(uv.data()), uv.size() / 2), kFacevaryingScope);
                ON3fGeomParam::Sample normals(N3fArraySample(reinterpret_cast<const N3f *>(n.data()), n.size() / 3), kFacevaryingScope);
                OPolyMeshSchema::Sample sample(V3fArraySample(positions, p.size() / 3), Int32ArraySample(indices), Int32ArraySample(counts),
                                               uv.empty() ? OV2fGeomParam::Sample() : uvs, n.empty() ? ON3fGeomParam::Sample() : normals);
                mesh.getSchema().set(sample);
            }
            if (color.valid()) {
                color.set(OC3fGeomParam::Sample(C3fArraySample(reinterpret_cast<const C3f *>(cd.data()), cd.size() / 3), color_scope));
            }
        }
        if (first) {
            throw std::runtime_error("nothing to write");
        }
        return 1;
    } catch (const std::exception &e) {
        *error = copy_string(e.what());
        return 0;
    }
}

void pfx_abc_free(char *text) { std::free(text); }

}
