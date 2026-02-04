use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use flate2::read::GzDecoder;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const INTERIOR1_TEMPLATE_PATH: &str = "assets/structures/interior_1.nbt";
const INTERIOR2_TEMPLATE_PATH: &str = "assets/structures/interior_2.nbt";
const INTERIOR1_TEMPLATE_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/structures/interior_1.nbt"
));
const INTERIOR2_TEMPLATE_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/structures/interior_2.nbt"
));
const GENERIC_WALL_BLOCK_ID: &str = "minecraft:bricks";

struct StructureTemplate {
    size: (i32, i32, i32),
    palette: Vec<PaletteEntry>,
    blocks: HashMap<(i32, i32, i32), TemplateBlock>,
    entities: Vec<TemplateEntity>,
}

struct PaletteEntry {
    name: String,
    properties: Option<Value>,
}

struct TemplateBlock {
    state: usize,
    nbt: Option<HashMap<String, Value>>,
}

struct TemplateEntity {
    pos: (f64, f64, f64),
    block_pos: (i32, i32, i32),
    nbt: HashMap<String, Value>,
}

struct ResolvedPaletteEntry {
    block: Option<BlockWithProperties>,
    is_air: bool,
    is_generic_wall: bool,
    is_door_lower: bool,
}

enum TemplateCache {
    Loaded(StructureTemplate),
    Failed(String),
}

static INTERIOR1_TEMPLATE: Lazy<TemplateCache> = Lazy::new(|| {
    load_structure_template_with_fallback(INTERIOR1_TEMPLATE_PATH, INTERIOR1_TEMPLATE_BYTES)
        .map(TemplateCache::Loaded)
        .unwrap_or_else(TemplateCache::Failed)
});

static INTERIOR2_TEMPLATE: Lazy<TemplateCache> = Lazy::new(|| {
    load_structure_template_with_fallback(INTERIOR2_TEMPLATE_PATH, INTERIOR2_TEMPLATE_BYTES)
        .map(TemplateCache::Loaded)
        .unwrap_or_else(TemplateCache::Failed)
});

static INTERIOR1_LOGGED: AtomicBool = AtomicBool::new(false);
static INTERIOR2_LOGGED: AtomicBool = AtomicBool::new(false);

fn get_template<'a>(
    cache: &'a TemplateCache,
    label: &str,
    logged: &AtomicBool,
) -> Option<&'a StructureTemplate> {
    match cache {
        TemplateCache::Loaded(template) => Some(template),
        TemplateCache::Failed(err) => {
            if !logged.swap(true, Ordering::Relaxed) {
                eprintln!("Failed to load interior template {label}: {err}");
            }
            None
        }
    }
}

fn load_structure_template_with_fallback(
    path: &str,
    fallback_bytes: &[u8],
) -> Result<StructureTemplate, String> {
    let candidates = candidate_paths(path);
    let mut errors = Vec::new();

    for candidate in candidates {
        let bytes = match std::fs::read(&candidate) {
            Ok(bytes) => bytes,
            Err(e) => {
                errors.push(format!("{}: {e}", candidate.display()));
                continue;
            }
        };

        match parse_structure_template_bytes(&bytes, &candidate.display().to_string()) {
            Ok(template) => return Ok(template),
            Err(e) => errors.push(e),
        }
    }

    parse_structure_template_bytes(fallback_bytes, "embedded template").map_err(|e| {
        if errors.is_empty() {
            e
        } else {
            format!("{e}. Previous attempts: {}", errors.join(" | "))
        }
    })
}

fn candidate_paths(path: &str) -> Vec<std::path::PathBuf> {
    let input = Path::new(path);
    if input.is_absolute() {
        return vec![input.to_path_buf()];
    }

    let mut paths = Vec::new();
    paths.push(input.to_path_buf());

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            paths.push(parent.join(input));
        }
    }

    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        paths.push(Path::new(manifest_dir).join(input));
    }

    paths
}

fn parse_structure_template_bytes(bytes: &[u8], label: &str) -> Result<StructureTemplate, String> {
    let nbt_bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = GzDecoder::new(bytes);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| format!("Failed to decompress {label}: {e}"))?;
        decompressed
    } else {
        bytes.to_vec()
    };

    let root_value: Value =
        fastnbt::from_bytes(&nbt_bytes).map_err(|e| format!("Failed to parse {label}: {e}"))?;

    let Value::Compound(root) = root_value else {
        return Err(format!(
            "Invalid structure template {label}: root is not a compound"
        ));
    };

    let size_value = root
        .get("size")
        .ok_or_else(|| format!("Invalid structure template {label}: missing size"))?;

    let (size_x, size_y, size_z) = parse_vec3_i32(size_value)
        .ok_or_else(|| format!("Invalid structure template {label}: invalid size"))?;

    let palette_value = if let Some(palette) = root.get("palette") {
        palette
    } else if let Some(Value::List(palettes)) = root.get("palettes") {
        palettes
            .get(0)
            .ok_or_else(|| format!("Invalid structure template {label}: palettes list is empty"))?
    } else {
        return Err(format!(
            "Invalid structure template {label}: missing palette/palettes"
        ));
    };

    let Value::List(palette_list) = palette_value else {
        return Err(format!(
            "Invalid structure template {label}: palette is not a list"
        ));
    };

    let mut palette = Vec::new();
    for entry in palette_list {
        let Value::Compound(map) = entry else {
            continue;
        };

        let Some(Value::String(name)) = map.get("Name") else {
            continue;
        };

        let properties = match map.get("Properties") {
            Some(Value::Compound(props)) => Some(Value::Compound(props.clone())),
            _ => None,
        };

        palette.push(PaletteEntry {
            name: name.clone(),
            properties,
        });
    }

    let mut blocks = HashMap::new();
    if let Some(Value::List(block_list)) = root.get("blocks") {
        for entry in block_list {
            let Value::Compound(map) = entry else {
                continue;
            };

            let Some(state_idx) = map.get("state").and_then(value_as_i32) else {
                continue;
            };
            if state_idx < 0 {
                continue;
            }

            let Some((x, y, z)) = map.get("pos").and_then(parse_vec3_i32) else {
                continue;
            };

            let nbt = match map.get("nbt") {
                Some(Value::Compound(nbt_map)) => Some(nbt_map.clone()),
                _ => None,
            };

            let state = state_idx as usize;
            blocks.insert((x, y, z), TemplateBlock { state, nbt });
        }
    }

    let mut entities = Vec::new();
    if let Some(Value::List(entity_list)) = root.get("entities") {
        for entry in entity_list {
            let Value::Compound(map) = entry else {
                continue;
            };

            let Some((x, y, z)) = map.get("pos").and_then(parse_vec3_f64) else {
                continue;
            };

            let block_pos = map.get("blockPos").and_then(parse_vec3_i32).unwrap_or((
                x.floor() as i32,
                y.floor() as i32,
                z.floor() as i32,
            ));

            let Some(Value::Compound(nbt_map)) = map.get("nbt") else {
                continue;
            };

            entities.push(TemplateEntity {
                pos: (x, y, z),
                block_pos,
                nbt: nbt_map.clone(),
            });
        }
    }

    Ok(StructureTemplate {
        size: (size_x, size_y, size_z),
        palette,
        blocks,
        entities,
    })
}

fn parse_vec3_i32(value: &Value) -> Option<(i32, i32, i32)> {
    match value {
        Value::List(list) => {
            if list.len() < 3 {
                return None;
            }
            Some((
                value_as_i32(&list[0])?,
                value_as_i32(&list[1])?,
                value_as_i32(&list[2])?,
            ))
        }
        Value::IntArray(array) => {
            let array = array.as_ref();
            if array.len() < 3 {
                return None;
            }
            Some((array[0], array[1], array[2]))
        }
        _ => None,
    }
}

fn parse_vec3_f64(value: &Value) -> Option<(f64, f64, f64)> {
    match value {
        Value::List(list) => {
            if list.len() < 3 {
                return None;
            }
            Some((
                value_as_f64(&list[0])?,
                value_as_f64(&list[1])?,
                value_as_f64(&list[2])?,
            ))
        }
        _ => None,
    }
}

fn value_as_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(v) => Some(*v as i32),
        Value::Short(v) => Some(*v as i32),
        Value::Int(v) => Some(*v),
        Value::Long(v) => i32::try_from(*v).ok(),
        _ => None,
    }
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Byte(v) => Some(*v as f64),
        Value::Short(v) => Some(*v as f64),
        Value::Int(v) => Some(*v as f64),
        Value::Long(v) => Some(*v as f64),
        Value::Float(v) => Some(*v as f64),
        Value::Double(v) => Some(*v),
        _ => None,
    }
}

fn strip_namespace(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn resolve_palette(
    palette: &[PaletteEntry],
    generic_wall_block_id: &str,
) -> Vec<ResolvedPaletteEntry> {
    let generic_wall = strip_namespace(generic_wall_block_id);

    palette
        .iter()
        .map(|entry| {
            let stripped = strip_namespace(&entry.name);
            let is_generic_wall = stripped == generic_wall;
            let is_air = matches!(stripped, "air" | "cave_air" | "void_air" | "structure_void");

            let is_door_lower = if stripped.ends_with("_door") {
                match entry.properties.as_ref() {
                    Some(Value::Compound(props)) => match props.get("half") {
                        Some(Value::String(half)) => half == "lower",
                        _ => true,
                    },
                    _ => true,
                }
            } else {
                false
            };

            let block = if is_air || is_generic_wall {
                None
            } else {
                block_from_name(stripped)
                    .map(|block| BlockWithProperties::new(block, entry.properties.clone()))
            };

            ResolvedPaletteEntry {
                block,
                is_air,
                is_generic_wall,
                is_door_lower,
            }
        })
        .collect()
}

fn place_template_entities(
    editor: &mut WorldEditor,
    template: &StructureTemplate,
    floor_area_set: &HashSet<(i32, i32)>,
    interior_min_x: i32,
    interior_min_z: i32,
    interior_max_x: i32,
    interior_max_z: i32,
    floor_y: i32,
    floor_index: i32,
    y_offset: i32,
    abs_terrain_offset: i32,
) {
    let (size_x, _size_y, size_z) = template.size;
    if size_x <= 0 || size_z <= 0 {
        return;
    }

    let offset_x = floor_index.rem_euclid(size_x);
    let offset_z = floor_index.rem_euclid(size_z);
    let origin_x = interior_min_x - offset_x;
    let origin_z = interior_min_z - offset_z;

    let mut tile_x = origin_x;
    while tile_x <= interior_max_x {
        let mut tile_z = origin_z;
        while tile_z <= interior_max_z {
            for entity in &template.entities {
                let world_block_x = tile_x + entity.block_pos.0;
                let world_block_z = tile_z + entity.block_pos.2;

                if world_block_x < interior_min_x
                    || world_block_x > interior_max_x
                    || world_block_z < interior_min_z
                    || world_block_z > interior_max_z
                {
                    continue;
                }

                if !floor_area_set.contains(&(world_block_x, world_block_z)) {
                    continue;
                }

                let world_pos = (
                    tile_x as f64 + entity.pos.0,
                    floor_y as f64 + y_offset as f64 + entity.pos.1 + abs_terrain_offset as f64,
                    tile_z as f64 + entity.pos.2,
                );

                editor.add_entity_absolute(entity.nbt.clone(), world_pos);
            }

            tile_z += size_z;
        }
        tile_x += size_x;
    }
}

/// Generates interior layouts inside buildings at each floor level
#[allow(clippy::too_many_arguments)]
pub fn generate_building_interior(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    start_y_offset: i32,
    building_height: i32,
    wall_block: Block,
    floor_levels: &[i32],
    args: &crate::args::Args,
    element: &crate::osm_parser::ProcessedWay,
    abs_terrain_offset: i32,
) {
    // Skip interior generation for very small buildings
    let width = max_x - min_x + 1;
    let depth = max_z - min_z + 1;

    if width < 8 || depth < 8 {
        return; // Building too small for interior
    }

    let Some(ground_template) = get_template(
        &INTERIOR1_TEMPLATE,
        INTERIOR1_TEMPLATE_PATH,
        &INTERIOR1_LOGGED,
    ) else {
        return;
    };
    let upper_template = get_template(
        &INTERIOR2_TEMPLATE,
        INTERIOR2_TEMPLATE_PATH,
        &INTERIOR2_LOGGED,
    );

    let resolved_ground = resolve_palette(&ground_template.palette, GENERIC_WALL_BLOCK_ID);
    let resolved_upper =
        upper_template.map(|template| resolve_palette(&template.palette, GENERIC_WALL_BLOCK_ID));

    // For efficiency, create a HashSet of floor area coordinates
    let floor_area_set: HashSet<(i32, i32)> = floor_area.iter().cloned().collect();

    // Add buffer around edges to avoid placing furniture too close to walls
    let buffer = 2;
    let interior_min_x = min_x + buffer;
    let interior_min_z = min_z + buffer;
    let interior_max_x = max_x - buffer;
    let interior_max_z = max_z - buffer;

    // Generate interiors for each floor
    for (floor_index, &floor_y) in floor_levels.iter().enumerate() {
        // Store wall and door positions for this floor to extend them to the ceiling
        let mut wall_positions = Vec::new();
        let mut door_positions = Vec::new();

        // Determine the floor extension height (ceiling) - either next floor or roof
        let current_floor_ceiling = if floor_index < floor_levels.len() - 1 {
            // For intermediate floors, extend walls up to just below the next floor
            floor_levels[floor_index + 1] - 1
        } else if args.roof
            && element.tags.contains_key("roof:shape")
            && element.tags.get("roof:shape").unwrap() != "flat"
        {
            // When roof generation is enabled with non-flat roofs, stop at building height (no extra ceiling)
            start_y_offset + building_height
        } else {
            // When roof generation is disabled or flat roof, extend to building top + 1 (includes ceiling)
            start_y_offset + building_height + 1
        };

        let (template, resolved_palette) = if floor_index == 0 || resolved_upper.is_none() {
            (ground_template, &resolved_ground)
        } else {
            (
                upper_template.expect("upper template checked"),
                resolved_upper.as_ref().expect("upper palette checked"),
            )
        };

        let (size_x, size_y, size_z) = template.size;
        if size_x <= 0 || size_y <= 0 || size_z <= 0 {
            continue;
        }

        // Calculate Y offset - place interior 1 block above floor level consistently
        let y_offset = 1;

        // Create a seamless repeating pattern across the interior of this floor
        for z in interior_min_z..=interior_max_z {
            for x in interior_min_x..=interior_max_x {
                // Skip if outside the building's floor area
                if !floor_area_set.contains(&(x, z)) {
                    continue;
                }

                // Map the world coordinates to pattern coordinates using modulo
                // This creates a seamless tiling effect across the entire building
                // Add floor_index offset to create variation between floors
                let pattern_x =
                    ((x - interior_min_x + floor_index as i32) % size_x + size_x) % size_x;
                let pattern_z =
                    ((z - interior_min_z + floor_index as i32) % size_z + size_z) % size_z;

                for y in 0..size_y {
                    let Some(template_block) = template.blocks.get(&(pattern_x, y, pattern_z))
                    else {
                        continue;
                    };

                    if template_block.state >= resolved_palette.len() {
                        continue;
                    }

                    let palette_entry = &resolved_palette[template_block.state];
                    if palette_entry.is_air {
                        continue;
                    }

                    let world_y = floor_y + y_offset + y + abs_terrain_offset;

                    let mut placed_block = false;
                    if palette_entry.is_generic_wall {
                        editor.set_block_absolute(wall_block, x, world_y, z, None, None);
                        placed_block = true;

                        if y == 0 {
                            wall_positions.push((x, z));
                        }
                    } else if let Some(block_with_props) = &palette_entry.block {
                        if block_with_props.properties.is_some() {
                            editor.set_block_with_properties_absolute(
                                block_with_props.clone(),
                                x,
                                world_y,
                                z,
                                None,
                                None,
                            );
                        } else {
                            editor.set_block_absolute(
                                block_with_props.block,
                                x,
                                world_y,
                                z,
                                None,
                                None,
                            );
                        }
                        placed_block = true;

                        if y == 0 && palette_entry.is_door_lower {
                            door_positions.push((x, z));
                        }
                    }

                    if placed_block {
                        if let Some(block_entity) = &template_block.nbt {
                            editor.add_block_entity_absolute(block_entity.clone(), x, world_y, z);
                        }
                    }
                }
            }
        }

        place_template_entities(
            editor,
            template,
            &floor_area_set,
            interior_min_x,
            interior_min_z,
            interior_max_x,
            interior_max_z,
            floor_y,
            floor_index as i32,
            y_offset,
            abs_terrain_offset,
        );

        // Extend walls all the way to the next floor ceiling or roof
        for (x, z) in &wall_positions {
            for y in (floor_y + y_offset + 2)..=current_floor_ceiling {
                editor.set_block_absolute(wall_block, *x, y + abs_terrain_offset, *z, None, None);
            }
        }

        // Add wall blocks above doors all the way to the ceiling/next floor
        for (x, z) in &door_positions {
            for y in (floor_y + y_offset + 2)..=current_floor_ceiling {
                editor.set_block_absolute(wall_block, *x, y + abs_terrain_offset, *z, None, None);
            }
        }
    }
}
