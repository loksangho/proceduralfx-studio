#include <openvdb/openvdb.h>
#include <openvdb/tools/MeshToVolume.h>
#include <openvdb/tools/Morphology.h>
#include <openvdb/tools/FastSweeping.h>
#include <openvdb/tools/VolumeToMesh.h>
#include <openvdb/tools/SignedFloodFill.h>
#include <openvdb/io/File.h>
#include <cstring>
#include <string>
#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <vector>

static void append_level_set_mesh(
    const openvdb::FloatGrid& grid,
    std::vector<float>& vertices_out,
    std::vector<unsigned>& triangles_out,
    double iso_value);

static void apply_level_set_operation(
    openvdb::FloatGrid::Ptr& grid,
    unsigned operation,
    unsigned iterations) {
    if (!grid || (operation != 1 && operation != 2)) return;
    (void)grid;
    (void)operation;
    (void)iterations;
}

extern "C" int mini_openvdb_surface(
    const float* vertices,
    size_t vertex_count,
    const unsigned* triangles,
    size_t triangle_count,
    float voxel_size,
    unsigned padding,
    unsigned operation,
    unsigned iterations,
    float** out_vertices,
    size_t* out_vertex_count,
    unsigned** out_triangles,
    size_t* out_triangle_count) {
    if (!vertices || !triangles || !out_vertices || !out_vertex_count ||
        !out_triangles || !out_triangle_count || voxel_size <= 0.0f) {
        return 0;
    }

    openvdb::initialize();
    std::vector<openvdb::Vec3s> points;
    std::vector<openvdb::Vec3I> faces;
    points.reserve(vertex_count);
    faces.reserve(triangle_count);
    for (size_t i = 0; i < vertex_count; ++i) {
        const float* v = vertices + i * 3;
        points.emplace_back(v[0], v[1], v[2]);
    }
    for (size_t i = 0; i < triangle_count; ++i) {
        const unsigned a = triangles[i * 3];
        const unsigned b = triangles[i * 3 + 1];
        const unsigned c = triangles[i * 3 + 2];
        if (a >= vertex_count || b >= vertex_count || c >= vertex_count) return 0;
        faces.emplace_back(a, b, c);
    }

    const auto transform =
        openvdb::math::Transform::createLinearTransform(voxel_size);
    auto grid = openvdb::tools::meshToLevelSet<openvdb::FloatGrid>(
        *transform, points, faces, 3.0f + static_cast<float>(padding));
    if (!grid) return 0;

    apply_level_set_operation(grid, operation, iterations);

    std::vector<float> vertices_out;
    std::vector<unsigned> triangles_out;
    const double iso = operation == 1
        ? static_cast<double>(iterations) * voxel_size
        : operation == 2
            ? -static_cast<double>(iterations) * voxel_size
            : 0.0;
    append_level_set_mesh(*grid, vertices_out, triangles_out, iso);

    *out_vertex_count = vertices_out.size() / 3;
    *out_triangle_count = triangles_out.size() / 3;
    *out_vertices = static_cast<float*>(std::malloc(vertices_out.size() * sizeof(float)));
    *out_triangles = static_cast<unsigned*>(std::malloc(triangles_out.size() * sizeof(unsigned)));
    if (!*out_vertices || !*out_triangles) {
        std::free(*out_vertices);
        std::free(*out_triangles);
        return 0;
    }
    std::copy(vertices_out.begin(), vertices_out.end(), *out_vertices);
    std::copy(triangles_out.begin(), triangles_out.end(), *out_triangles);
    return 1;
}

struct VolumeHandle {
    openvdb::FloatGrid::Ptr grid;
    double iso = 0.0;
};

static void append_level_set_mesh(
    const openvdb::FloatGrid& grid,
    std::vector<float>& vertices_out,
    std::vector<unsigned>& triangles_out,
    double iso_value) {
    std::vector<openvdb::Vec3s> points;
    std::vector<openvdb::Vec3I> triangles;
    std::vector<openvdb::Vec4I> quads;
    openvdb::tools::volumeToMesh(grid, points, triangles, quads, iso_value);
    vertices_out.reserve(points.size() * 3);
    triangles_out.reserve((triangles.size() + quads.size() * 2) * 3);
    for (const auto& point : points) {
        vertices_out.insert(vertices_out.end(), {
            point.x(), point.y(), point.z()
        });
    }
    for (const auto& triangle : triangles) {
        triangles_out.insert(triangles_out.end(), {
            static_cast<unsigned>(triangle[0]),
            static_cast<unsigned>(triangle[1]),
            static_cast<unsigned>(triangle[2])
        });
    }
    for (const auto& quad : quads) {
        triangles_out.insert(triangles_out.end(), {
            static_cast<unsigned>(quad[0]),
            static_cast<unsigned>(quad[1]),
            static_cast<unsigned>(quad[2]),
            static_cast<unsigned>(quad[0]),
            static_cast<unsigned>(quad[2]),
            static_cast<unsigned>(quad[3])
        });
    }
}

extern "C" void* mini_openvdb_volume_from_mesh(
    const float* vertices, size_t vertex_count, const unsigned* triangles,
    size_t triangle_count, float voxel_size, unsigned padding) {
    if (!vertices || !triangles || voxel_size <= 0.0f) return nullptr;
    std::vector<openvdb::Vec3s> points;
    std::vector<openvdb::Vec3I> faces;
    points.reserve(vertex_count);
    faces.reserve(triangle_count);
    for (size_t i = 0; i < vertex_count; ++i) {
        const float* v = vertices + i * 3;
        points.emplace_back(v[0], v[1], v[2]);
    }
    for (size_t i = 0; i < triangle_count; ++i) {
        const unsigned a = triangles[i * 3];
        const unsigned b = triangles[i * 3 + 1];
        const unsigned c = triangles[i * 3 + 2];
        if (a >= vertex_count || b >= vertex_count || c >= vertex_count) return nullptr;
        faces.emplace_back(a, b, c);
    }
    const auto transform =
        openvdb::math::Transform::createLinearTransform(voxel_size);
    auto grid = openvdb::tools::meshToLevelSet<openvdb::FloatGrid>(
        *transform, points, faces, 3.0f + static_cast<float>(padding));
    return new VolumeHandle{grid, 0.0};
}

extern "C" void* mini_openvdb_volume_clone(void* handle) {
    if (!handle) return nullptr;
    const auto* source = static_cast<VolumeHandle*>(handle);
    return new VolumeHandle{source->grid->deepCopy(), source->iso};
}

extern "C" int mini_openvdb_volume_operation(
    void* handle, unsigned operation, unsigned iterations) {
    if (!handle) return 0;
    auto* volume = static_cast<VolumeHandle*>(handle);
    if (operation == 1) {
        volume->iso += static_cast<double>(iterations) * volume->grid->voxelSize()[0];
    } else if (operation == 2) {
        volume->iso -= static_cast<double>(iterations) * volume->grid->voxelSize()[0];
    }
    return 1;
}

extern "C" void mini_openvdb_volume_free(void* handle) {
    delete static_cast<VolumeHandle*>(handle);
}

extern "C" int mini_openvdb_volume_to_mesh(
    void* handle, float** out_vertices, size_t* out_vertex_count,
    unsigned** out_triangles, size_t* out_triangle_count) {
    if (!handle) return 0;
    auto* volume = static_cast<VolumeHandle*>(handle);
    std::vector<float> vertices_out;
    std::vector<unsigned> triangles_out;
    append_level_set_mesh(*volume->grid, vertices_out, triangles_out, volume->iso);
    *out_vertex_count = vertices_out.size() / 3;
    *out_triangle_count = triangles_out.size() / 3;
    *out_vertices = static_cast<float*>(std::malloc(vertices_out.size() * sizeof(float)));
    *out_triangles = static_cast<unsigned*>(std::malloc(triangles_out.size() * sizeof(unsigned)));
    if (!*out_vertices || !*out_triangles) {
        std::free(*out_vertices);
        std::free(*out_triangles);
        return 0;
    }
    std::copy(vertices_out.begin(), vertices_out.end(), *out_vertices);
    std::copy(triangles_out.begin(), triangles_out.end(), *out_triangles);
    return 1;
}

extern "C" void mini_openvdb_free(void* pointer) {
    std::free(pointer);
}


// ---- Voxel access and .vdb files (Convert VDB, File and File Export nodes) ----

static char* copy_message(const std::string& text) {
    char* out = static_cast<char*>(std::malloc(text.size() + 1));
    if (out) std::memcpy(out, text.c_str(), text.size() + 1);
    return out;
}

// Copies a grid's active voxels: index coordinates (xyz per voxel) and values minus `shift`.
static int export_active_voxels(const openvdb::FloatGrid& grid, double shift, int** out_coords,
                                float** out_values, size_t* out_count) {
    const size_t count = grid.activeVoxelCount();
    *out_count = count;
    *out_coords = static_cast<int*>(std::malloc(std::max<size_t>(count, 1) * 3 * sizeof(int)));
    *out_values = static_cast<float*>(std::malloc(std::max<size_t>(count, 1) * sizeof(float)));
    if (!*out_coords || !*out_values) {
        std::free(*out_coords);
        std::free(*out_values);
        return 0;
    }
    size_t i = 0;
    for (auto it = grid.cbeginValueOn(); it && i < count; ++it, ++i) {
        const openvdb::Coord c = it.getCoord();
        (*out_coords)[i * 3] = c.x();
        (*out_coords)[i * 3 + 1] = c.y();
        (*out_coords)[i * 3 + 2] = c.z();
        (*out_values)[i] = static_cast<float>(*it - shift);
    }
    *out_count = i;
    return 1;
}

// Active voxels of a VDB node's level set (world position = index * voxel size).
extern "C" int mini_openvdb_volume_voxels(void* handle, int** out_coords, float** out_values,
                                          size_t* out_count, float* out_voxel, float* out_background) {
    if (!handle) return 0;
    auto* volume = static_cast<VolumeHandle*>(handle);
    *out_voxel = static_cast<float>(volume->grid->voxelSize()[0]);
    *out_background = volume->grid->background();
    return export_active_voxels(*volume->grid, volume->iso, out_coords, out_values, out_count);
}

// Reads the "density" or "surface" float grid of a .vdb file (else its first float grid).
extern "C" int mini_openvdb_read(const char* path, int** out_coords, float** out_values, size_t* out_count,
                                 float* out_voxel, float* out_origin, float* out_background,
                                 int* out_level_set, char** out_error) {
    try {
        openvdb::initialize();
        openvdb::io::File file(path);
        file.open();
        openvdb::GridPtrVecPtr grids = file.getGrids();
        file.close();
        openvdb::FloatGrid::Ptr chosen;
        for (const auto& base : *grids) {
            auto grid = openvdb::gridPtrCast<openvdb::FloatGrid>(base);
            if (!grid) continue;
            if (!chosen || grid->getName() == "density" || grid->getName() == "surface") chosen = grid;
            if (grid->getName() == "density" || grid->getName() == "surface") break;
        }
        if (!chosen) {
            *out_error = copy_message("the file has no float grids");
            return 0;
        }
        *out_voxel = static_cast<float>(chosen->voxelSize()[0]);
        const openvdb::Vec3d origin = chosen->indexToWorld(openvdb::Coord(0, 0, 0));
        out_origin[0] = static_cast<float>(origin.x());
        out_origin[1] = static_cast<float>(origin.y());
        out_origin[2] = static_cast<float>(origin.z());
        *out_background = chosen->background();
        *out_level_set = chosen->getGridClass() == openvdb::GRID_LEVEL_SET ? 1 : 0;
        return export_active_voxels(*chosen, 0.0, out_coords, out_values, out_count);
    } catch (const std::exception& e) {
        *out_error = copy_message(e.what());
        return 0;
    }
}

// Writes voxels as a level set ("surface") or fog volume ("density") grid.
extern "C" int mini_openvdb_write(const char* path, const int* coords, const float* values, size_t count,
                                  float voxel, const float* origin, float background, int level_set,
                                  char** out_error) {
    try {
        openvdb::initialize();
        auto grid = openvdb::FloatGrid::create(background);
        auto transform = openvdb::math::Transform::createLinearTransform(voxel);
        transform->postTranslate(openvdb::Vec3d(origin[0], origin[1], origin[2]));
        grid->setTransform(transform);
        auto accessor = grid->getAccessor();
        for (size_t i = 0; i < count; ++i) {
            accessor.setValueOn(openvdb::Coord(coords[i * 3], coords[i * 3 + 1], coords[i * 3 + 2]), values[i]);
        }
        grid->setGridClass(level_set ? openvdb::GRID_LEVEL_SET : openvdb::GRID_FOG_VOLUME);
        grid->setName(level_set ? "surface" : "density");
        if (level_set) openvdb::tools::signedFloodFill(grid->tree());
        openvdb::io::File file(path);
        file.write({grid});
        file.close();
        return 1;
    } catch (const std::exception& e) {
        *out_error = copy_message(e.what());
        return 0;
    }
}

// A fog volume for rendering: "density" float grid plus an optional "emission" colour grid.
extern "C" int mini_openvdb_write_volume(const char* path, const int* coords, const float* density,
                                         const float* emission, size_t count, float voxel,
                                         const float* origin, char** out_error) {
    try {
        openvdb::initialize();
        auto transform = openvdb::math::Transform::createLinearTransform(voxel);
        transform->postTranslate(openvdb::Vec3d(origin[0], origin[1], origin[2]));
        auto density_grid = openvdb::FloatGrid::create(0.0f);
        density_grid->setTransform(transform);
        density_grid->setName("density");
        density_grid->setGridClass(openvdb::GRID_FOG_VOLUME);
        auto density_access = density_grid->getAccessor();
        openvdb::Vec3SGrid::Ptr emission_grid;
        if (emission) {
            emission_grid = openvdb::Vec3SGrid::create(openvdb::Vec3s(0.0f));
            emission_grid->setTransform(transform->copy());
            emission_grid->setName("emission");
        }
        for (size_t i = 0; i < count; ++i) {
            const openvdb::Coord c(coords[i * 3], coords[i * 3 + 1], coords[i * 3 + 2]);
            density_access.setValueOn(c, density[i]);
            if (emission_grid) {
                const float* e = emission + i * 3;
                if (e[0] > 0.0f || e[1] > 0.0f || e[2] > 0.0f) {
                    emission_grid->tree().setValueOn(c, openvdb::Vec3s(e[0], e[1], e[2]));
                }
            }
        }
        openvdb::GridPtrVec grids{density_grid};
        if (emission_grid) grids.push_back(emission_grid);
        openvdb::io::File file(path);
        file.write(grids);
        file.close();
        return 1;
    } catch (const std::exception& e) {
        *out_error = copy_message(e.what());
        return 0;
    }
}
