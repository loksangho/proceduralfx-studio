# Scene file node reference

Generated from the code by `node_reference_is_current` (see [scene-format.md](scene-format.md)). Each `node` line stores its parameters in the order below, as numbers; the values shown are a new node's defaults. Nodes with text fields store them first, as `t:` followed by hex-encoded UTF-8 strings joined by `|`.

## USD Create

| Node | Saved as | Parameters |
| --- | --- | --- |
| sop_import | `sop_import` | 1 text field(s); #0=-1 |
| usd_reference | `usd_reference` | 2 text field(s); sublayer=0 |
| usd_primitive | `usd_primitive` | 1 text field(s); shape=0, size=1, t.x=0, t.y=1, t.z=0 |
| usd_light | `usd_light` | 1 text field(s); type=0, position.x=4, position.y=6, position.z=4, target.x=0, target.y=0, target.z=0, color.r=1, color.g=0.97, color.b=0.9, intensity=1, size=1 |
| usd_camera | `usd_camera` | 1 text field(s); position.x=5, position.y=4, position.z=6, target.x=0, target.y=0, target.z=0, focal=35, aperture=36 |

## USD Edit

| Node | Saved as | Parameters |
| --- | --- | --- |
| usd_transform | `usd_transform` | 1 text field(s); t.x=0, t.y=0, t.z=0, r.x=0, r.y=0, r.z=0, s.x=1, s.y=1, s.z=1 |
| assign_material | `assign_material` | 1 text field(s); #0=-1 |
| usd_prune | `usd_prune` | 1 text field(s); hide=0 |
| usd_merge | `usd_merge` | none |
| usd_export | `usd_export` | 1 text field(s) |

## Objects

| Node | Saved as | Parameters |
| --- | --- | --- |
| geometry | `geometry` | #0=-1 |
| camera | `camera` | position.x=0, position.y=0, position.z=0, rotation.x=23, rotation.y=-35, rotation.z=0, focal=1.2, near=0.5, orthographic=0, pan.x=0, pan.y=0, zoom=6 |
| light | `light` | #0=3, #1=4, #2=3, #3=-35, #4=35, #5=0, #6=1, #7=0.95, #8=0.85, #9=0.9, #10=0 |
| environment_light | `environment_light` | 1 text field(s); #0=1, #1=1, #2=1, #3=1, #4=0, #5=1 |

## Generators

| Node | Saved as | Parameters |
| --- | --- | --- |
| add | `add` | point_count=0, spacing=1, layout=0, delete_geometry=0, #4=0, #5=0, #6=0 |
| grid | `grid` | size=4, resolution=20 |
| plane | `plane` | size=4, resolution=20 |
| cone | `cone` | radius=1, height=2, rows=1, columns=32, caps=1 |
| capsule | `capsule` | radius=0.6, length=1.5, rows=16, columns=32 |
| box | `box` | size=1.5, divisions=4 |
| sphere | `sphere` | radius=1, rows=16, columns=32 |
| torus | `torus` | major_radius=1, minor_radius=0.35, rows=16, columns=32 |
| tube | `tube` | bottom_radius=0.5, top_radius=0.5, height=2, rows=8, columns=24, caps=1 |
| lsystem | `lsystem` | 2 text field(s); generations=6, angle=28, step=0.6, step_scale=0.78, thickness=0.06, thickness_scale=0.7, angle_random=0, seed=1 |
| test_geometry | `test_geometry` | shape=0, size=2, on_ground=1, colours=1, p=2, q=3, detail=48, clip=1, look=0, looping=1 |
| particle_surface | `particle_surface` | particle_radius=0.12, join_distance=0.08, resolution=48, smoothing=0 |

## Groom

| Node | Saved as | Parameters |
| --- | --- | --- |
| hair_guides | `hair_guides` | count=200, length=0.6, length_variance=0.2, segments=8, droop=0.3, seed=1 |
| guide_process | `guide_process` | operation=1, amount=0.5, frequency=2, seed=1 |
| hair_generate | `hair_generate` | count=5000, segments=8, clump=0.3, length_variance=0.15, frizz=0.2, root_width=0.004, tip_width=0.001, seed=1 |
| hair_deform | `hair_deform` | rest_frame=1 |
| attribute_paint | `attribute_paint` | 1 text field(s); paint_value=1, default_value=0, brush_radius=40, brush_strength=0.5, mode=0, visualize=1 |

## Solvers

| Node | Saved as | Parameters |
| --- | --- | --- |
| particles | `particles` | max_particles=2000, birth_rate=400, lifetime=4, speed=2, spread=0.35, gravity=-1.5, seed=1, drag=0, bounce=0.4, friction=0.1, radius=0.02, substeps=2, along_normals=0 |
| flip | `flip` | particles=20000, grid_resolution=64, flip_blend=0.95, viscosity=0.1, gravity=-9.8, max_timestep=0.02, substeps=2, seed=11, domain_center.x=0, domain_center.y=0, domain_center.z=0, domain_size.x=0, domain_size.y=0, domain_size.z=0 |
| whitewater_solver | `whitewater_solver` | max_particles=200000, emission_rate=400, trapped_air=1, wave_crests=1, min_speed=1, full_emission_speed=6, foam_lifespan=2, spray_drag=0.3, bubble_buoyancy=2, bubble_drag=0.5, spray_neighbours=6, bubble_neighbours=20, gravity=-9.8, particle_size=0.01, substeps=1, seed=1 |
| pyro | `pyro` | resolution=40, center.x=0, center.y=2, center.z=0, size.x=4, size.y=5, size.z=4, substeps=1, density=1, temperature=1, fuel=0, buoyancy=1, dissipation=0.15, cooling=0.8, vorticity=0.4, turbulence=0.3, burn_rate=1, wind.x=0, wind.y=0, wind.z=0, opacity=1 |
| rbd_solver | `rbd_solver` | shape=0, density=1, friction=0.6, bounce=0.2, gravity=9.81, substeps=2, ground=1, ground_height=0, velocity.x=0, velocity.y=0, velocity.z=0, spin.x=0, spin.y=0, spin.z=0 |
| cloth_solver | `cloth_solver` | type=0, stretch_stiffness=100000, bend_stiffness=1, shape_stiffness=0.5, mass=1, thickness=0.02, gravity=-9.8, damping=0.1, friction=0.3, substeps=16, iterations=4, ground=1, self_collision=0, pin_to_animation=1 |
| sop_solver | `sop_solver` | substeps=1 |
| time_shift | `time_shift` | offset=0, hold=0, at=0 |
| time_warp | `time_warp` | speed=0.5, anchor=0 |
| time_blend | `time_blend` | none |
| trail | `trail` | samples=5, step=1, mode=0, fade=1 |
| loop_particles | `loop_particles` | start=1, length=1 |
| make_loop | `make_loop` | start=2, length=1, blend=0.5 |
| solver_input | `solver_input` | #0=0 |

## Import/Export

| Node | Saved as | Parameters |
| --- | --- | --- |
| file | `file` | 2 text field(s); loop=0 |
| file_export | `file_export` | 1 text field(s) |
| object_merge | `object_merge` | into_this_object=1, #1=0 |

## Volumes

| Node | Saved as | Parameters |
| --- | --- | --- |
| vdb | `vdb` | voxel_size=0.1, padding=0, operation=0, iterations=1, output=1 |
| convert_vdb | `convert_vdb` | mode=0, iso=0 |
| volume_wrangle | `volume_wrangle` | 1 text field(s) |

## Deformers

| Node | Saved as | Parameters |
| --- | --- | --- |
| xform | `xform` | tx=0, ty=0, tz=0, rx=0, ry=0, rz=0, sx=1, sy=1, sz=1 |
| noise | `noise` | frequency=2.2, amplitude=0.3 |
| taper | `taper` | taper=0.3 |
| twist | `twist` | twist_angle=1 |
| bend | `bend` | bend_angle=0.5 |
| point_deform | `point_deform` | influences=4, fit_neighbours=6, follow_rotation=1 |
| transform_pieces | `transform_pieces` | none |
| smooth | `smooth` | iterations=5, strength=0.5 |
| bevel | `bevel` | amount=0.25, segments=2 |
| peak | `peak` | distance=0.1 |
| ripple | `ripple` | frequency=6, amplitude=0.3, speed=4 |
| point_jitter | `jitter` | amount=0.05, seed=1, scale.x=1, scale.y=1, scale.z=1 |
| match_size | `match_size` | justify.x=1, justify.y=0, justify.z=1, scale_to_fit=0, uniform=1, center.x=0, center.y=0, center.z=0, size.x=1, size.y=1, size.z=1 |

## Topology

| Node | Saved as | Parameters |
| --- | --- | --- |
| subdivide | `subdivide` | levels=1 |
| remesh | `remesh` | target_triangles=2000 |
| fuse | `fuse` | distance=0.001 |
| mirror | `mirror` | axis=0 |
| copy | `copy` | copies=5, t.x=1.5, t.y=0, t.z=0, r.x=0, r.y=0, r.z=0, uniform_scale=1 |
| copy_to_points | `copy_to_points` | scale=1, r.x=0, r.y=0, r.z=0 |
| extrude | `extrude` | 3 text field(s); distance=0.2, inset=0, connected=1, divisions=1, output_front=1, output_back=0, output_side=1, output_groups=1 |
| clip | `clip` | axis=1, offset=0, invert=0 |
| blast | `blast` | component=1, invert=0, #2=0 |
| scatter | `scatter` | count=1000, seed=1, relax_iterations=5, fill_volume=0 |
| boolean | `boolean` | operation=2, method=0, voxel_resolution=128 |
| pack | `pack` | pack=1, pivot=0 |
| unpack | `unpack` | transfer_attributes=1 |
| cache | `cache` | #0=1, mode=1, start=1, end=100, step=1 |
| reverse | `reverse` | none |
| extract_centroid | `extract_centroid` | partition=1, method=0 |
| convert_line | `convert_line` | none |
| resample | `resample` | by_length=1, length=0.1, segments=10 |
| metaball | `metaball` | radius=1, threshold=0.3, resolution=48 |
| switch | `switch` | input=0 |
| merge | `merge` | none |
| null | `null` | none |

## Flow Control

| Node | Saved as | Parameters |
| --- | --- | --- |
| For-Each Connected Piece | `foreach_end` | 1 text field(s); loop_over=0, iterations=10, start_value=0, increment=1, single_pass=-1, multithread=1, #6=-1 |
| For-Each Named Primitive | `foreach_end` | 1 text field(s); loop_over=1, iterations=10, start_value=0, increment=1, single_pass=-1, multithread=1, #6=-1 |
| For-Each Point | `foreach_end` | 1 text field(s); loop_over=2, iterations=10, start_value=0, increment=1, single_pass=-1, multithread=1, #6=-1 |
| For-Each Primitive | `foreach_end` | 1 text field(s); loop_over=3, iterations=10, start_value=0, increment=1, single_pass=-1, multithread=1, #6=-1 |
| For-Each Number | `foreach_end` | 1 text field(s); loop_over=4, iterations=10, start_value=0, increment=1, single_pass=-1, multithread=1, #6=-1 |
| For Loop with Feedback | `foreach_end` | 1 text field(s); loop_over=5, iterations=10, start_value=0, increment=1, single_pass=-1, multithread=1, #6=-1 |
| Compile Block | `compile_end` | #0=-1 |

## Shading

| Node | Saved as | Parameters |
| --- | --- | --- |
| material | `material` | 1 text field(s); #0=0 |

## Destruction

| Node | Saved as | Parameters |
| --- | --- | --- |
| material_fracture (Concrete) | `material_fracture` | material=0, pieces=20, seed=1, impact.x=0, impact.y=0, impact.z=0, crack_rings=4, radial_cracks=9, grain_axis=1, splinter_length=5, explode=0, glue_strength=200 |
| material_fracture (Glass) | `material_fracture` | material=1, pieces=20, seed=1, impact.x=0, impact.y=0, impact.z=0, crack_rings=4, radial_cracks=9, grain_axis=1, splinter_length=5, explode=0, glue_strength=200 |
| material_fracture (Wood) | `material_fracture` | material=2, pieces=20, seed=1, impact.x=0, impact.y=0, impact.z=0, crack_rings=4, radial_cracks=9, grain_axis=1, splinter_length=5, explode=0, glue_strength=200 |
| rbd_configure | `rbd_configure` | density=1, friction=0.6, bounce=0.2, active=1, velocity.x=0, velocity.y=0, velocity.z=0, #7=1, #8=1, #9=1, #10=1, #11=0 |
| rbd_bullet_solver | `rbd_bullet_solver` | density=1, friction=0.6, bounce=0.2, gravity=9.81, substeps=4, ground=1, ground_height=0, glue_strength_scale=1, velocity.x=0, velocity.y=0, velocity.z=0 |

## Crowds

| Node | Saved as | Parameters |
| --- | --- | --- |
| crowd_solver | `crowd_solver` | speed=1.4, speed_variance=0.2, turn_rate=120, personal_space=0.6, separation=2, alignment=0.3, cohesion=0.1, goal.x=0, goal.y=0, goal.z=0, goal_weight=0, wander=0.5, obstacle_lookahead=1.5, agent_height=1.75, display=1, seed=1 |

## Rigging

| Node | Saved as | Parameters |
| --- | --- | --- |
| skeleton | `skeleton` | preset=0, scale=1, joints=6, length=2 |
| rig_pose | `rig_pose` | 8 text field(s); #0=0, #1=0, #2=0, #3=0, #4=0, #5=0, #6=0, #7=0, #8=0, #9=0, #10=0, #11=0, #12=0, #13=0, #14=0, #15=0, #16=0, #17=0, #18=0, #19=0, #20=0, #21=0, #22=0, #23=0, #24=0, #25=0, #26=0, #27=0, #28=0, #29=0, #30=0, #31=0, #32=0, #33=0, #34=0, #35=0, #36=0, #37=0, #38=0, #39=0, #40=0, #41=0, #42=0, #43=0, #44=0, #45=0, #46=0, #47=0 |
| two_bone_ik | `two_bone_ik` | 3 text field(s); goal.x=0.1, goal.y=0.3, goal.z=0.3, pole.x=0.1, pole.y=0.5, pole.z=1, use_pole=1, blend=1, keep_tip=1 |
| procedural_motion | `procedural_motion` | motion=0, cycles_per_second=1, amplitude=1, forward_speed=1.2, lag=0.6, axis=0 |
| skeleton_blend | `skeleton_blend` | weight=0.5 |
| joint_capture | `joint_capture` | joints_per_point=3, falloff=4, colour_by_joint=0 |
| bone_deform | `bone_deform` | none |

## Muscles

| Node | Saved as | Parameters |
| --- | --- | --- |
| muscle | `muscle` | origin.x=0, origin.y=1, origin.z=0, insertion.x=0, insertion.y=0, insertion.z=0, radius=0.12, shape=0, flatness=3, tendon=0.15, activation=0, contraction=0.3, segments=24, sides=16, twist=0, muscle_id=0 |
| muscle_solver | `muscle_solver` | firmness=120, damping=6, coherence=4, substeps=4 |
| muscle_skin | `muscle_skin` | reach=0.3, influences=4 |

## Terrain

| Node | Saved as | Parameters |
| --- | --- | --- |
| terrain | `terrain` | size.x=20, size.z=20, resolution=256 |
| terrain_noise | `terrain_noise` | noise=0, blend=0, height=1.5, element_size=4, octaves=6, roughness=0.5, offset.x=0, offset.z=0, seed=1, positive=1, use_mask=0 |
| terrain_erode | `terrain_erode` | raindrops=50000, erosion_rate=0.3, deposition_rate=0.3, sediment_capacity=4, evaporation=0.02, inertia=0.05, erosion_radius=2, thermal_iterations=10, talus_angle=40, seed=1 |
| terrain_terrace | `terrain_terrace` | step_height=0.5, offset=0, sharpness=0.6, use_mask=0 |
| terrain_mask | `terrain_mask` | source=1, min=30, max=90, falloff=5, blur=1, invert=0, combine=0, visualize=1 |
| terrain_colorize | `terrain_colorize` | sand.r=0.76, sand.g=0.7, sand.b=0.5, grass.r=0.24, grass.g=0.4, grass.b=0.16, rock.r=0.42, rock.g=0.38, rock.b=0.34, snow.r=0.95, snow.g=0.96, snow.b=0.98, sand_level=0.08, snow_level=0.75, rock_slope=35, blend=0.05 |
| heightfield_wrangle | `heightfield_wrangle` | 1 text field(s) |

## Attributes

| Node | Saved as | Parameters |
| --- | --- | --- |
| attribute_create | `attribute_create` | 1 text field(s); #0=4, value.x=1, class=0, type=3, size=3, value.y=1, value.z=1, value.w=0 |
| attribute_randomize | `attribute_randomize` | 1 text field(s); #0=4, amount=1, seed=1, class=0, size=3, min.x=0, max.x=1, mode=0, type=3, distribution=0, min.y=0, min.z=0, min.w=0, max.y=1, max.z=1, max.w=1 |
| attribute_delete | `attribute_delete` | 1 text field(s); #0=4, class=0 |
| color | `color` | 1 text field(s); class=0, mode=0, color.r=1, color.g=0.45, color.b=0.1, color2.r=0.1, color2.g=0.3, color2.b=1, seed=1, axis=1 |
| attribute_transfer | `attribute_transfer` | 1 text field(s); class=0, samples=4, distance=1, blend_width=0 |
| wrangle | `wrangle` | the run-over domain (0–3), `:`, then the code in hex |
| attribute_vop | `attribute_vop` | run_over=2 |
| normal | `normal` | domain=1, cusp_angle=60, weighting=0, reverse=0 |
| measure | `measure` | 2 text field(s); measure=0, curvature=0, summary=1 |
| connectivity | `connectivity` | 1 text field(s); class=1 |

## Groups

| Node | Saved as | Parameters |
| --- | --- | --- |
| group_create | `group_create` | 2 text field(s); primitives=0, method=0, bbox_min.x=-1, bbox_min.y=-1, bbox_min.z=-1, bbox_max.x=1, bbox_max.y=1, bbox_max.z=1, percent=50, seed=1, direction.x=0, direction.y=1, direction.z=0, spread=30, combine=0, invert=0 |

## UV

| Node | Saved as | Parameters |
| --- | --- | --- |
| uv_texture | `uv_texture` | projection=3, axis=1, scale.u=1, scale.v=1, offset.u=0, offset.v=0 |
| uv_transform | `uv_transform` | translate.u=0, translate.v=0, rotate=0, scale.u=1, scale.v=1, pivot.u=0.5, pivot.v=0.5 |
| uv_quickshade | `uv_quickshade` | 1 text field(s) |

## Materials

| Node | Saved as | Parameters |
| --- | --- | --- |
| diffuse | `diffuse` | #0=0, color.r=0.8, color.g=0.8, color.b=0.8, roughness=0.5, value=0.5 |
| metal | `metal` | #0=1, color.r=0.9, color.g=0.7, color.b=0.3, roughness=0.3, value=0 |
| glass | `glass` | #0=2, color.r=1, color.g=1, color.b=1, roughness=0.02, value=1.5 |
| emissive | `emissive` | #0=3, color.r=1, color.g=0.9, color.b=0.7, roughness=0, value=1 |

## Shaders

| Node | Saved as | Parameters |
| --- | --- | --- |
| Principled | `principled_shader` | 3 text field(s); #0=0, base_color.r=0.8, base_color.g=0.8, base_color.b=0.8, use_point_color=0, metallic=0, roughness=0.4, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Toon Shader | `toon_shader` | #0=0, base_color.r=0.9, base_color.g=0.35, base_color.b=0.25, shadow_color.r=0.25, shadow_color.g=0.2, shadow_color.b=0.35, bands=2, softness=0, terminator=0.05, highlight=1, highlight_size=0.3, highlight_color.r=1, highlight_color.g=1, highlight_color.b=1, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0 |
| Hair Shader | `hair_shader` | #0=0, hair_color.r=0.3, hair_color.g=0.17, hair_color.b=0.08, roughness=0.35, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |
| Pyro Shader | `pyro_shader` | #0=0, smoke_color.r=0.2, smoke_color.g=0.2, smoke_color.b=0.2, smoke_density=1, anisotropy=0.3, fire_color.r=1, fire_color.g=1, fire_color.b=1, fire_intensity=1 |
| material_builder | `material_builder` | none |

## Render

| Node | Saved as | Parameters |
| --- | --- | --- |
| moonray_render | `moonray_render` | `mr:` then 20 fields separated by `\|` (see scene-format.md) |

## Liquids & Glass

| Node | Saved as | Parameters |
| --- | --- | --- |
| Water | `principled_shader` | 3 text field(s); #0=1, base_color.r=1, base_color.g=1, base_color.b=1, use_point_color=0, metallic=0, roughness=0.02, specular=1, ior=1.333, transmission=1, transmission_color.r=0.86, transmission_color.g=0.95, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Glass | `principled_shader` | 3 text field(s); #0=2, base_color.r=1, base_color.g=1, base_color.b=1, use_point_color=0, metallic=0, roughness=0, specular=1, ior=1.5, transmission=1, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Frosted Glass | `principled_shader` | 3 text field(s); #0=3, base_color.r=1, base_color.g=1, base_color.b=1, use_point_color=0, metallic=0, roughness=0.3, specular=1, ior=1.5, transmission=1, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Ice | `principled_shader` | 3 text field(s); #0=4, base_color.r=0.8, base_color.g=0.8, base_color.b=0.8, use_point_color=0, metallic=0, roughness=0.12, specular=1, ior=1.31, transmission=1, transmission_color.r=0.88, transmission_color.g=0.96, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Diamond | `principled_shader` | 3 text field(s); #0=5, base_color.r=1, base_color.g=1, base_color.b=1, use_point_color=0, metallic=0, roughness=0, specular=1, ior=2.42, transmission=1, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Honey | `principled_shader` | 3 text field(s); #0=6, base_color.r=0.8, base_color.g=0.8, base_color.b=0.8, use_point_color=0, metallic=0, roughness=0.05, specular=1, ior=1.5, transmission=1, transmission_color.r=1, transmission_color.g=0.55, transmission_color.b=0.08, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Soap Bubble | `principled_shader` | 3 text field(s); #0=7, base_color.r=0.8, base_color.g=0.8, base_color.b=0.8, use_point_color=0, metallic=0, roughness=0, specular=1, ior=1.33, transmission=1, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=1, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=1, texture_scale_u=1, texture_scale_v=1 |

## Metals

| Node | Saved as | Parameters |
| --- | --- | --- |
| Gold | `principled_shader` | 3 text field(s); #0=8, base_color.r=1, base_color.g=0.77, base_color.b=0.34, use_point_color=0, metallic=1, roughness=0.18, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Silver | `principled_shader` | 3 text field(s); #0=9, base_color.r=0.97, base_color.g=0.96, base_color.b=0.91, use_point_color=0, metallic=1, roughness=0.12, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Copper | `principled_shader` | 3 text field(s); #0=10, base_color.r=0.95, base_color.g=0.64, base_color.b=0.54, use_point_color=0, metallic=1, roughness=0.25, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Brass | `principled_shader` | 3 text field(s); #0=11, base_color.r=0.91, base_color.g=0.78, base_color.b=0.42, use_point_color=0, metallic=1, roughness=0.25, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Aluminum | `principled_shader` | 3 text field(s); #0=12, base_color.r=0.91, base_color.g=0.92, base_color.b=0.92, use_point_color=0, metallic=1, roughness=0.3, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Brushed Aluminum | `principled_shader` | 3 text field(s); #0=13, base_color.r=0.91, base_color.g=0.92, base_color.b=0.92, use_point_color=0, metallic=1, roughness=0.35, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0.75, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Chrome | `principled_shader` | 3 text field(s); #0=14, base_color.r=0.62, base_color.g=0.63, base_color.b=0.64, use_point_color=0, metallic=1, roughness=0.03, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Steel | `principled_shader` | 3 text field(s); #0=15, base_color.r=0.56, base_color.g=0.57, base_color.b=0.58, use_point_color=0, metallic=1, roughness=0.35, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Mirror | `principled_shader` | 3 text field(s); #0=16, base_color.r=0.96, base_color.g=0.96, base_color.b=0.97, use_point_color=0, metallic=1, roughness=0, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |

## Plastics & Paint

| Node | Saved as | Parameters |
| --- | --- | --- |
| Plastic | `principled_shader` | 3 text field(s); #0=17, base_color.r=0.8, base_color.g=0.12, base_color.b=0.1, use_point_color=0, metallic=0, roughness=0.3, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Glossy Plastic | `principled_shader` | 3 text field(s); #0=18, base_color.r=0.1, base_color.g=0.3, base_color.b=0.8, use_point_color=0, metallic=0, roughness=0.08, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0.5, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Rubber | `principled_shader` | 3 text field(s); #0=19, base_color.r=0.04, base_color.g=0.04, base_color.b=0.04, use_point_color=0, metallic=0, roughness=0.75, specular=0.4, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Car Paint | `principled_shader` | 3 text field(s); #0=20, base_color.r=0.55, base_color.g=0.02, base_color.b=0.03, use_point_color=0, metallic=0, roughness=0.35, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=1, clearcoat_roughness=0.02, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |

## Ceramics & Stone

| Node | Saved as | Parameters |
| --- | --- | --- |
| Porcelain | `principled_shader` | 3 text field(s); #0=21, base_color.r=0.95, base_color.g=0.94, base_color.b=0.9, use_point_color=0, metallic=0, roughness=0.1, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0.25, subsurface_color.r=1, subsurface_color.g=0.95, subsurface_color.b=0.9, subsurface_radius=0.3, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=1, clearcoat_roughness=0.02, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Ceramic | `principled_shader` | 3 text field(s); #0=22, base_color.r=0.72, base_color.g=0.36, base_color.b=0.2, use_point_color=0, metallic=0, roughness=0.25, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0.8, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Marble | `principled_shader` | 3 text field(s); #0=23, base_color.r=0.92, base_color.g=0.91, base_color.b=0.88, use_point_color=0, metallic=0, roughness=0.1, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0.6, subsurface_color.r=0.95, subsurface_color.g=0.9, subsurface_color.b=0.85, subsurface_radius=0.5, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Jade | `principled_shader` | 3 text field(s); #0=24, base_color.r=0.3, base_color.g=0.6, base_color.b=0.35, use_point_color=0, metallic=0, roughness=0.15, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=1, subsurface_color.r=0.4, subsurface_color.g=0.9, subsurface_color.b=0.5, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Concrete | `principled_shader` | 3 text field(s); #0=25, base_color.r=0.55, base_color.g=0.55, base_color.b=0.52, use_point_color=0, metallic=0, roughness=0.9, specular=0.3, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Clay | `principled_shader` | 3 text field(s); #0=26, base_color.r=0.7, base_color.g=0.42, base_color.b=0.3, use_point_color=0, metallic=0, roughness=0.85, specular=0.2, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |

## Organic

| Node | Saved as | Parameters |
| --- | --- | --- |
| Skin | `principled_shader` | 3 text field(s); #0=27, base_color.r=0.85, base_color.g=0.6, base_color.b=0.5, use_point_color=0, metallic=0, roughness=0.45, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=1, subsurface_color.r=1, subsurface_color.g=0.35, subsurface_color.b=0.2, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Wax | `principled_shader` | 3 text field(s); #0=28, base_color.r=0.95, base_color.g=0.9, base_color.b=0.75, use_point_color=0, metallic=0, roughness=0.3, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=1, subsurface_color.r=1, subsurface_color.g=0.8, subsurface_color.b=0.5, subsurface_radius=2, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Milk | `principled_shader` | 3 text field(s); #0=29, base_color.r=0.95, base_color.g=0.95, base_color.b=0.92, use_point_color=0, metallic=0, roughness=0.15, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=1, subsurface_color.r=1, subsurface_color.g=0.97, subsurface_color.b=0.9, subsurface_radius=2, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Chocolate | `principled_shader` | 3 text field(s); #0=30, base_color.r=0.25, base_color.g=0.12, base_color.b=0.06, use_point_color=0, metallic=0, roughness=0.3, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0.3, subsurface_color.r=0.6, subsurface_color.g=0.3, subsurface_color.b=0.1, subsurface_radius=0.3, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Wood | `principled_shader` | 3 text field(s); #0=31, base_color.r=0.45, base_color.g=0.28, base_color.b=0.15, use_point_color=0, metallic=0, roughness=0.55, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0.3, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Leather | `principled_shader` | 3 text field(s); #0=32, base_color.r=0.3, base_color.g=0.15, base_color.b=0.08, use_point_color=0, metallic=0, roughness=0.5, specular=0.5, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Mud | `principled_shader` | 3 text field(s); #0=33, base_color.r=0.25, base_color.g=0.18, base_color.b=0.12, use_point_color=0, metallic=0, roughness=0.7, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0.3, clearcoat_roughness=0.2, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |

## Cloth & Paper

| Node | Saved as | Parameters |
| --- | --- | --- |
| Velvet | `principled_shader` | 3 text field(s); #0=34, base_color.r=0.4, base_color.g=0.02, base_color.b=0.1, use_point_color=0, metallic=0, roughness=0.9, specular=0.2, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=1, sheen_color.r=1, sheen_color.g=0.5, sheen_color.b=0.6, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Cotton | `principled_shader` | 3 text field(s); #0=35, base_color.r=0.8, base_color.g=0.8, base_color.b=0.78, use_point_color=0, metallic=0, roughness=0.9, specular=0.3, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0.5, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Paper | `principled_shader` | 3 text field(s); #0=36, base_color.r=0.9, base_color.g=0.9, base_color.b=0.86, use_point_color=0, metallic=0, roughness=0.9, specular=0.3, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0.2, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=0.2, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |

## Emissive

| Node | Saved as | Parameters |
| --- | --- | --- |
| Lava | `principled_shader` | 3 text field(s); #0=37, base_color.r=0.05, base_color.g=0.04, base_color.b=0.04, use_point_color=0, metallic=0, roughness=0.8, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=0.35, emission_color.b=0.05, emission_intensity=8, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Neon | `principled_shader` | 3 text field(s); #0=38, base_color.r=0.1, base_color.g=0.1, base_color.b=0.1, use_point_color=0, metallic=0, roughness=0.4, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=0.2, emission_color.g=0.8, emission_color.b=1, emission_intensity=20, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |

## Toon & Hair Presets

| Node | Saved as | Parameters |
| --- | --- | --- |
| Anime | `toon_shader` | #0=1, base_color.r=0.9, base_color.g=0.35, base_color.b=0.25, shadow_color.r=0.25, shadow_color.g=0.2, shadow_color.b=0.35, bands=2, softness=0, terminator=0.05, highlight=1.2, highlight_size=0.15, highlight_color.r=1, highlight_color.g=1, highlight_color.b=1, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0 |
| Cel 3 Tone | `toon_shader` | #0=2, base_color.r=0.9, base_color.g=0.35, base_color.b=0.25, shadow_color.r=0.25, shadow_color.g=0.2, shadow_color.b=0.35, bands=3, softness=0, terminator=0.05, highlight=1, highlight_size=0.3, highlight_color.r=1, highlight_color.g=1, highlight_color.b=1, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0 |
| Soft Toon | `toon_shader` | #0=3, base_color.r=0.9, base_color.g=0.35, base_color.b=0.25, shadow_color.r=0.25, shadow_color.g=0.2, shadow_color.b=0.35, bands=3, softness=1, terminator=0.05, highlight=0.6, highlight_size=0.5, highlight_color.r=1, highlight_color.g=1, highlight_color.b=1, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0 |
| Toon Metal | `toon_shader` | #0=4, base_color.r=0.8, base_color.g=0.8, base_color.b=0.85, shadow_color.r=0.15, shadow_color.g=0.15, shadow_color.b=0.25, bands=4, softness=0, terminator=0.05, highlight=2, highlight_size=0.08, highlight_color.r=1, highlight_color.g=1, highlight_color.b=1, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0 |
| Brown Hair | `hair_shader` | #0=1, hair_color.r=0.3, hair_color.g=0.17, hair_color.b=0.08, roughness=0.35, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |
| Blonde Hair | `hair_shader` | #0=2, hair_color.r=0.75, hair_color.g=0.58, hair_color.b=0.33, roughness=0.35, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |
| Black Hair | `hair_shader` | #0=3, hair_color.r=0.03, hair_color.g=0.02, hair_color.b=0.015, roughness=0.25, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |
| Red Hair | `hair_shader` | #0=4, hair_color.r=0.55, hair_color.g=0.15, hair_color.b=0.05, roughness=0.35, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |
| Grey Hair | `hair_shader` | #0=5, hair_color.r=0.75, hair_color.g=0.75, hair_color.b=0.75, roughness=0.35, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |
| Fur | `hair_shader` | #0=6, hair_color.r=0.45, hair_color.g=0.32, hair_color.b=0.2, roughness=0.55, ior=1.55, specular_tint.r=1, specular_tint.g=1, specular_tint.b=1, glint=0 |

## Volume Presets

| Node | Saved as | Parameters |
| --- | --- | --- |
| Fire | `pyro_shader` | #0=1, smoke_color.r=0.12, smoke_color.g=0.12, smoke_color.b=0.12, smoke_density=1, anisotropy=0.3, fire_color.r=1, fire_color.g=0.85, fire_color.b=0.7, fire_intensity=2 |
| Smoke | `pyro_shader` | #0=2, smoke_color.r=0.55, smoke_color.g=0.55, smoke_color.b=0.55, smoke_density=1, anisotropy=0.3, fire_color.r=1, fire_color.g=1, fire_color.b=1, fire_intensity=0 |
| Explosion | `pyro_shader` | #0=3, smoke_color.r=0.08, smoke_color.g=0.08, smoke_color.b=0.08, smoke_density=1.5, anisotropy=0.3, fire_color.r=1, fire_color.g=0.6, fire_color.b=0.3, fire_intensity=6 |
| Clouds | `pyro_shader` | #0=4, smoke_color.r=0.95, smoke_color.g=0.95, smoke_color.b=0.97, smoke_density=0.5, anisotropy=0.6, fire_color.r=1, fire_color.g=1, fire_color.b=1, fire_intensity=0 |
| Steam | `pyro_shader` | #0=5, smoke_color.r=0.9, smoke_color.g=0.9, smoke_color.b=0.9, smoke_density=0.3, anisotropy=0.5, fire_color.r=1, fire_color.g=1, fire_color.b=1, fire_intensity=0 |

## COP Network

| Node | Saved as | Parameters |
| --- | --- | --- |
| cop_network | `cop_network` | none |

## Generators

| Node | Saved as | Parameters |
| --- | --- | --- |
| File | `cop` | 1 text field(s); operation=0 |
| Render | `cop` | operation=1 |
| Constant | `cop` | operation=2, color.r=0.18, color.g=0.18, color.b=0.18, alpha=1, width=1280, height=720 |
| Ramp | `cop` | operation=3, color_a.r=0, color_a.g=0, color_a.b=0, color_b.r=1, color_b.g=1, color_b.b=1, type=0, width=1280, height=720 |
| Noise | `cop` | operation=4, color_a.r=0, color_a.g=0, color_a.b=0, color_b.r=1, color_b.g=1, color_b.b=1, size=120, octaves=4, speed=0, seed=0, width=1280, height=720 |

## Composite

| Node | Saved as | Parameters |
| --- | --- | --- |
| Composite | `cop` | operation=5, mode=0, mix=1 |
| Chroma Key | `cop` | operation=18, key.r=0.1, key.g=0.8, key.b=0.2, tolerance=0.25, softness=0.15, despill=1 |
| Switch | `cop` | operation=20, index=0 |

## Color

| Node | Saved as | Parameters |
| --- | --- | --- |
| Color Correct | `cop` | operation=6, exposure=0, contrast=1, gamma=1, saturation=1, hue=0, gain.r=1, gain.g=1, gain.b=1, offset.r=0, offset.g=0, offset.b=0 |
| Invert | `cop` | operation=7 |
| Levels | `cop` | operation=8, in_black=0, in_white=1, out_black=0, out_white=1 |

## Filters

| Node | Saved as | Parameters |
| --- | --- | --- |
| Blur | `cop` | operation=9, radius=4 |
| Sharpen | `cop` | operation=10, amount=0.8, radius=1.5 |
| Glow | `cop` | operation=11, threshold=0.8, radius=20, intensity=1 |
| Edge Detect | `cop` | operation=12, strength=1 |
| Vignette | `cop` | operation=13, amount=0.5, softness=0.6 |
| Grain | `cop` | operation=14, amount=0.05, seed=0 |

## Transform

| Node | Saved as | Parameters |
| --- | --- | --- |
| Transform | `cop` | operation=15, tx=0, ty=0, rotate=0, scale=1 |
| Crop | `cop` | operation=16, left=0, right=0, top=0, bottom=0 |
| Resize | `cop` | operation=17, width=1280, height=720 |

## Output

| Node | Saved as | Parameters |
| --- | --- | --- |
| File Output | `cop` | 1 text field(s); operation=19 |

## TOP Network

| Node | Saved as | Parameters |
| --- | --- | --- |
| top_network | `top_network` | none |

## Tasks

| Node | Saved as | Parameters |
| --- | --- | --- |
| Frame Range | `top` | operation=0, start=1, end=48, step=1 |
| Wedge | `top` | 1 text field(s); operation=1, count=3, start=0, end=1, #4=-1, #5=0 |
| Fetch | `top` | 1 text field(s); operation=2, #1=-1 |
| Command | `top` | 2 text field(s); operation=3 |
| File Pattern | `top` | 1 text field(s); operation=4 |
| Wait for All | `top` | operation=5 |
| FFmpeg Encode | `top` | 1 text field(s); operation=6, fps=24 |

## Shader Nodes

| Node | Saved as | Parameters |
| --- | --- | --- |
| Surface Output | `shader_node` | operation=0, #1=0, base_color.r=0.8, base_color.g=0.8, base_color.b=0.8, use_point_color=0, metallic=0, roughness=0.4, specular=1, ior=1.5, transmission=0, transmission_color.r=1, transmission_color.g=1, transmission_color.b=1, thin_walled=0, subsurface=0, subsurface_color.r=1, subsurface_color.g=0.45, subsurface_color.b=0.3, subsurface_radius=1, sheen=0, sheen_color.r=1, sheen_color.g=1, sheen_color.b=1, sheen_roughness=0.3, clearcoat=0, clearcoat_roughness=0.05, emission_color.r=1, emission_color.g=1, emission_color.b=1, emission_intensity=0, opacity=1, anisotropy=0, iridescence=0, texture_scale_u=1, texture_scale_v=1 |
| Image Texture | `shader_node` | 1 text field(s); operation=1, repeat_u=1, repeat_v=1, tint.r=1, tint.g=1, tint.b=1, color_data=1 |
| Normal Map | `shader_node` | 1 text field(s); operation=2, repeat_u=1, repeat_v=1 |
| Point Attribute | `shader_node` | 1 text field(s); operation=3, is_float=0, default.r=1, default.g=1, default.b=1 |
| Cells | `shader_node` | operation=5, color_a.r=0, color_a.g=0, color_a.b=0, color_b.r=1, color_b.g=1, color_b.b=1, frequency=4, jitter=1, edges=0, seed=0 |
| Checker | `shader_node` | operation=6, color_a.r=0.1, color_a.g=0.1, color_a.b=0.1, color_b.r=0.9, color_b.g=0.9, color_b.b=0.9, tiles_u=8, tiles_v=8 |
| Color Ramp | `shader_node` | operation=7, color_a.r=0, color_a.g=0, color_a.b=0, color_b.r=1, color_b.g=1, color_b.b=1 |
| Mix | `shader_node` | operation=8, color_a.r=0, color_a.g=0, color_a.b=0, color_b.r=1, color_b.g=1, color_b.b=1, factor=0.5 |
| HSV Adjust | `shader_node` | operation=9, color.r=0.5, color.g=0.5, color.b=0.5, hue=0, saturation=1, value=1 |

## Inputs

| Node | Saved as | Parameters |
| --- | --- | --- |
| Bind | `vop` | 1 text field(s); operation=0, #1=1, #2=0, #3=0, #4=0 |
| Parameter | `vop` | 1 text field(s); operation=2, #1=0, #2=0, #3=0, #4=0 |

## Math

| Node | Saved as | Parameters |
| --- | --- | --- |
| Add | `vop` | 1 text field(s); operation=7, #1=0, #2=0, #3=0, #4=0 |
| Subtract | `vop` | 1 text field(s); operation=8, #1=0, #2=0, #3=0, #4=0 |
| Multiply | `vop` | 1 text field(s); operation=9, #1=0, #2=0, #3=0, #4=0 |
| Divide | `vop` | 1 text field(s); operation=10, #1=0, #2=0, #3=0, #4=0 |
| Negate | `vop` | 1 text field(s); operation=11, #1=0, #2=0, #3=0, #4=0 |
| Absolute | `vop` | 1 text field(s); operation=12, #1=0, #2=0, #3=0, #4=0 |
| Floor | `vop` | 1 text field(s); operation=13, #1=0, #2=0, #3=0, #4=0 |
| Fraction | `vop` | 1 text field(s); operation=14, #1=0, #2=0, #3=0, #4=0 |
| Power | `vop` | 1 text field(s); operation=15, #1=0, #2=0, #3=0, #4=0 |
| Square Root | `vop` | 1 text field(s); operation=16, #1=0, #2=0, #3=0, #4=0 |
| Sine | `vop` | 1 text field(s); operation=17, #1=0, #2=0, #3=0, #4=0 |
| Cosine | `vop` | 1 text field(s); operation=18, #1=0, #2=0, #3=0, #4=0 |
| Minimum | `vop` | 1 text field(s); operation=19, #1=0, #2=0, #3=0, #4=0 |
| Maximum | `vop` | 1 text field(s); operation=20, #1=0, #2=0, #3=0, #4=0 |
| Clamp | `vop` | 1 text field(s); operation=22, #1=0, #2=0, #3=0, #4=0 |
| Fit Range | `vop` | 1 text field(s); operation=23, #1=0, #2=0, #3=0, #4=0 |
| Ramp Smooth | `vop` | 1 text field(s); operation=24, #1=0, #2=0, #3=0, #4=0 |
| Modulo | `vop` | 1 text field(s); operation=48, #1=0, #2=0, #3=0, #4=0 |
| Round | `vop` | 1 text field(s); operation=49, #1=0, #2=0, #3=0, #4=0 |
| Ceiling | `vop` | 1 text field(s); operation=50, #1=0, #2=0, #3=0, #4=0 |
| Exponential | `vop` | 1 text field(s); operation=51, #1=0, #2=0, #3=0, #4=0 |
| Logarithm | `vop` | 1 text field(s); operation=52, #1=0, #2=0, #3=0, #4=0 |
| Tangent | `vop` | 1 text field(s); operation=53, #1=0, #2=0, #3=0, #4=0 |
| Arctangent2 | `vop` | 1 text field(s); operation=54, #1=0, #2=0, #3=0, #4=0 |
| Average | `vop` | 1 text field(s); operation=55, #1=0, #2=0, #3=0, #4=0 |
| Sign | `vop` | 1 text field(s); operation=56, #1=0, #2=0, #3=0, #4=0 |

## Vector

| Node | Saved as | Parameters |
| --- | --- | --- |
| Component | `vop` | 1 text field(s); operation=4, #1=0, #2=0, #3=0, #4=0 |
| Dot Product | `vop` | 1 text field(s); operation=25, #1=0, #2=0, #3=0, #4=0 |
| Cross Product | `vop` | 1 text field(s); operation=26, #1=0, #2=0, #3=0, #4=0 |
| Length | `vop` | 1 text field(s); operation=27, #1=0, #2=0, #3=0, #4=0 |
| Normalize | `vop` | 1 text field(s); operation=28, #1=0, #2=0, #3=0, #4=0 |
| Distance | `vop` | 1 text field(s); operation=29, #1=0, #2=0, #3=0, #4=0 |
| Make Vector | `vop` | 1 text field(s); operation=30, #1=0, #2=0, #3=0, #4=0 |
| Rotate | `vop` | 1 text field(s); operation=31, #1=0, #2=0, #3=0, #4=0 |
| Reflect | `vop` | 1 text field(s); operation=57, #1=0, #2=0, #3=0, #4=0 |

## Geometry

| Node | Saved as | Parameters |
| --- | --- | --- |
| Get Attribute | `vop` | 1 text field(s); operation=38, #1=1, #2=0, #3=0, #4=0 |
| Point Cloud Average | `vop` | 1 text field(s); operation=39, #1=1, #2=0, #3=0, #4=0 |
| Nearest Point | `vop` | 1 text field(s); operation=40, #1=0, #2=0, #3=0, #4=0 |
| Distance to Surface | `vop` | 1 text field(s); operation=41, #1=0, #2=0, #3=0, #4=0 |
| Closest Point on Surface | `vop` | 1 text field(s); operation=42, #1=0, #2=0, #3=0, #4=0 |
| Neighbour Count | `vop` | 1 text field(s); operation=43, #1=0, #2=0, #3=0, #4=0 |
| Point Count | `vop` | 1 text field(s); operation=44, #1=0, #2=0, #3=0, #4=0 |
| Bounding Box Centre | `vop` | 1 text field(s); operation=45, #1=0, #2=0, #3=0, #4=0 |
| Bounding Box Size | `vop` | 1 text field(s); operation=46, #1=0, #2=0, #3=0, #4=0 |
| Relative to Bounding Box | `vop` | 1 text field(s); operation=47, #1=0, #2=0, #3=0, #4=0 |

## Noise

| Node | Saved as | Parameters |
| --- | --- | --- |
| Signed Noise | `vop` | 1 text field(s); operation=33, #1=0, #2=0, #3=0, #4=0 |
| Curl Noise | `vop` | 1 text field(s); operation=34, #1=0, #2=0, #3=0, #4=0 |
| Worley Noise | `vop` | 1 text field(s); operation=35, #1=0, #2=0, #3=0, #4=0 |
| Random | `vop` | 1 text field(s); operation=36, #1=0, #2=0, #3=0, #4=0 |
| Alligator Noise | `vop` | 1 text field(s); operation=58, #1=0, #2=0, #3=0, #4=0 |

## Logic

| Node | Saved as | Parameters |
| --- | --- | --- |
| Compare | `vop` | 1 text field(s); operation=5, #1=0, #2=0, #3=0, #4=0 |
| Snippet | `vop` | 1 text field(s); operation=6, #1=0, #2=0, #3=0, #4=0 |
| And | `vop` | 1 text field(s); operation=59, #1=0, #2=0, #3=0, #4=0 |
| Or | `vop` | 1 text field(s); operation=60, #1=0, #2=0, #3=0, #4=0 |
| Not | `vop` | 1 text field(s); operation=61, #1=0, #2=0, #3=0, #4=0 |

## Outputs

| Node | Saved as | Parameters |
| --- | --- | --- |
| Export | `vop` | 1 text field(s); operation=3, #1=1, #2=0, #3=0, #4=0 |
