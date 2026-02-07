use crate::block_definitions::{
    block_from_java_name, Block, BlockWithProperties, CHISELLED_BOOKSHELF,
};
use crate::structure_template::StructureTemplate;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const INTERIOR_Y_OFFSET: i32 = 1;

struct TemplateConfig {
    path: &'static str,
    generic_wall_block: &'static str,
    embedded: &'static [u8],
}

const INTERIOR_1_BYTES: &[u8] = include_bytes!("../../../assets/structures/interior_1.nbt");
const INTERIOR_2_BYTES: &[u8] = include_bytes!("../../../assets/structures/interior_2.nbt");
const ABANDONED_INTERIOR_1_BYTES: &[u8] =
    include_bytes!("../../../assets/structures/abandoned_interior_1.nbt");
const ABANDONED_INTERIOR_2_BYTES: &[u8] =
    include_bytes!("../../../assets/structures/abandoned_interior_2.nbt");

const INTERIOR_1_CONFIG: TemplateConfig = TemplateConfig {
    path: "assets/structures/interior_1.nbt",
    generic_wall_block: "minecraft:bricks",
    embedded: INTERIOR_1_BYTES,
};

const INTERIOR_2_CONFIG: TemplateConfig = TemplateConfig {
    path: "assets/structures/interior_2.nbt",
    generic_wall_block: "minecraft:bricks",
    embedded: INTERIOR_2_BYTES,
};

const ABANDONED_INTERIOR_1_CONFIG: TemplateConfig = TemplateConfig {
    path: "assets/structures/abandoned_interior_1.nbt",
    generic_wall_block: "minecraft:bricks",
    embedded: ABANDONED_INTERIOR_1_BYTES,
};

const ABANDONED_INTERIOR_2_CONFIG: TemplateConfig = TemplateConfig {
    path: "assets/structures/abandoned_interior_2.nbt",
    generic_wall_block: "minecraft:bricks",
    embedded: ABANDONED_INTERIOR_2_BYTES,
};

enum PaletteMapping {
    Skip,
    GenericWall,
    Block(Block, Option<Value>),
}

struct InteriorTemplate {
    template: StructureTemplate,
    palette_mapping: Vec<PaletteMapping>,
    door_lower_states: Vec<bool>,
}

static INTERIOR_1_TEMPLATE: OnceLock<Option<InteriorTemplate>> = OnceLock::new();
static INTERIOR_2_TEMPLATE: OnceLock<Option<InteriorTemplate>> = OnceLock::new();
static ABANDONED_INTERIOR_1_TEMPLATE: OnceLock<Option<InteriorTemplate>> = OnceLock::new();
static ABANDONED_INTERIOR_2_TEMPLATE: OnceLock<Option<InteriorTemplate>> = OnceLock::new();

fn get_template(
    cache: &'static OnceLock<Option<InteriorTemplate>>,
    config: &TemplateConfig,
) -> Option<&'static InteriorTemplate> {
    cache.get_or_init(|| load_template(config)).as_ref()
}

fn load_template(config: &TemplateConfig) -> Option<InteriorTemplate> {
    let template = match StructureTemplate::from_file_or_bytes(config.path, config.embedded) {
        Ok(template) => template,
        Err(err) => {
            eprintln!("{err}");
            return None;
        }
    };

    let (palette_mapping, door_lower_states) =
        build_palette_mapping(&template, config.generic_wall_block, config.path);

    Some(InteriorTemplate {
        template,
        palette_mapping,
        door_lower_states,
    })
}

fn build_palette_mapping(
    template: &StructureTemplate,
    generic_wall_block: &str,
    template_path: &str,
) -> (Vec<PaletteMapping>, Vec<bool>) {
    let normalized_wall = normalize_block_name(generic_wall_block);
    let mut palette_mapping = Vec::with_capacity(template.palette.len());
    let mut door_lower_states = Vec::with_capacity(template.palette.len());
    let mut unknown_blocks = Vec::new();

    for entry in &template.palette {
        let name = normalize_block_name(&entry.name);
        let is_air = name == "air";
        let is_generic_wall = name == normalized_wall;
        let is_door_lower = is_lower_door(name, &entry.properties);

        let mapping = if is_air {
            PaletteMapping::Skip
        } else if is_generic_wall {
            PaletteMapping::GenericWall
        } else if let Some(block) = block_from_java_name(&entry.name) {
            PaletteMapping::Block(block, entry.properties.clone())
        } else if name == "chiseled_bookshelf" {
            PaletteMapping::Block(CHISELLED_BOOKSHELF, entry.properties.clone())
        } else {
            unknown_blocks.push(entry.name.clone());
            PaletteMapping::Skip
        };

        palette_mapping.push(mapping);
        door_lower_states.push(is_door_lower);
    }

    if !unknown_blocks.is_empty() {
        eprintln!(
            "Unknown blocks in interior template {}: {}",
            template_path,
            unknown_blocks.join(", ")
        );
    }

    (palette_mapping, door_lower_states)
}

fn normalize_block_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn is_lower_door(name: &str, properties: &Option<Value>) -> bool {
    if !name.ends_with("_door") {
        return false;
    }

    let Some(Value::Compound(props)) = properties else {
        return false;
    };

    match props.get("half") {
        Some(Value::String(half)) => half == "lower",
        _ => false,
    }
}

fn place_template_for_floor(
    editor: &mut WorldEditor,
    template: &InteriorTemplate,
    floor_area_set: &HashSet<(i32, i32)>,
    interior_min_x: i32,
    interior_min_z: i32,
    interior_max_x: i32,
    interior_max_z: i32,
    floor_index: usize,
    floor_y: i32,
    abs_terrain_offset: i32,
    wall_block: Block,
    wall_positions: &mut HashSet<(i32, i32)>,
    door_positions: &mut HashSet<(i32, i32)>,
) {
    let (size_x, size_y, size_z) = template.template.size;
    if size_x <= 0 || size_z <= 0 || size_y <= 0 {
        return;
    }

    let base_origin_x = interior_min_x - floor_index as i32;
    let base_origin_z = interior_min_z - floor_index as i32;

    let start_tile_x = base_origin_x + (interior_min_x - base_origin_x).div_euclid(size_x) * size_x;
    let end_tile_x = base_origin_x + (interior_max_x - base_origin_x).div_euclid(size_x) * size_x;
    let start_tile_z = base_origin_z + (interior_min_z - base_origin_z).div_euclid(size_z) * size_z;
    let end_tile_z = base_origin_z + (interior_max_z - base_origin_z).div_euclid(size_z) * size_z;

    let base_y = floor_y + INTERIOR_Y_OFFSET;
    let base_y_absolute = base_y + abs_terrain_offset;

    for tile_z in (start_tile_z..=end_tile_z).step_by(size_z as usize) {
        for tile_x in (start_tile_x..=end_tile_x).step_by(size_x as usize) {
            for template_block in &template.template.blocks {
                if template_block.pos.0 < 0
                    || template_block.pos.1 < 0
                    || template_block.pos.2 < 0
                    || template_block.pos.0 >= size_x
                    || template_block.pos.1 >= size_y
                    || template_block.pos.2 >= size_z
                {
                    continue;
                }

                let state = template_block.state;
                if state >= template.palette_mapping.len() {
                    continue;
                }

                let world_x = tile_x + template_block.pos.0;
                let world_z = tile_z + template_block.pos.2;
                if world_x < interior_min_x
                    || world_x > interior_max_x
                    || world_z < interior_min_z
                    || world_z > interior_max_z
                {
                    continue;
                }

                if !floor_area_set.contains(&(world_x, world_z)) {
                    continue;
                }

                let local_y = template_block.pos.1;
                let world_y_absolute = base_y_absolute + local_y;

                let mapping = &template.palette_mapping[state];

                match mapping {
                    PaletteMapping::Skip => {}
                    PaletteMapping::GenericWall => {
                        editor.set_block_absolute(
                            wall_block,
                            world_x,
                            world_y_absolute,
                            world_z,
                            None,
                            None,
                        );

                        if local_y == 0 {
                            wall_positions.insert((world_x, world_z));
                        }
                    }
                    PaletteMapping::Block(block, props) => {
                        let mut block_props = props.clone();
                        let mut block_entity = template_block.nbt.clone();

                        if is_chiseled_bookshelf(*block) {
                            if let Some(ref mut nbt) = block_entity {
                                if let Some(occupied) = normalize_chiseled_bookshelf_nbt(nbt) {
                                    apply_bookshelf_slot_properties(&mut block_props, occupied);
                                }
                            }
                        }

                        if let Some(props) = &block_props {
                            editor.set_block_with_properties_absolute(
                                BlockWithProperties::new(*block, Some(props.clone())),
                                world_x,
                                world_y_absolute,
                                world_z,
                                None,
                                None,
                            );
                        } else {
                            editor.set_block_absolute(
                                *block,
                                world_x,
                                world_y_absolute,
                                world_z,
                                None,
                                None,
                            );
                        }

                        if local_y == 0 && template.door_lower_states[state] {
                            door_positions.insert((world_x, world_z));
                        }

                        if let Some(nbt) = block_entity {
                            editor.add_block_entity_absolute(
                                world_x,
                                world_y_absolute,
                                world_z,
                                nbt,
                            );
                        }
                    }
                }
            }

            for entity in &template.template.entities {
                let world_x = tile_x + entity.block_pos.0;
                let world_z = tile_z + entity.block_pos.2;
                if world_x < interior_min_x
                    || world_x > interior_max_x
                    || world_z < interior_min_z
                    || world_z > interior_max_z
                {
                    continue;
                }

                if !floor_area_set.contains(&(world_x, world_z)) {
                    continue;
                }

                let world_block_y_absolute = base_y_absolute + entity.block_pos.1;
                let world_pos = (
                    tile_x as f64 + entity.pos.0,
                    base_y_absolute as f64 + entity.pos.1,
                    tile_z as f64 + entity.pos.2,
                );

                editor.add_entity_nbt_absolute(
                    entity.nbt.clone(),
                    world_pos,
                    (world_x, world_block_y_absolute, world_z),
                );
            }
        }
    }
}

fn is_chiseled_bookshelf(block: Block) -> bool {
    block.name() == "chiseled_bookshelf"
}

fn normalize_chiseled_bookshelf_nbt(nbt: &mut HashMap<String, Value>) -> Option<[bool; 6]> {
    let Some(Value::List(items)) = nbt.get_mut("Items") else {
        return None;
    };

    let mut occupied = [false; 6];

    for item in items.iter_mut() {
        let Value::Compound(item_map) = item else {
            continue;
        };

        if let Some(slot) = item_map.get("Slot").and_then(value_to_i32) {
            if (0..6).contains(&slot) {
                occupied[slot as usize] = true;
            }
        }

        normalize_item_stack(item_map);
    }

    Some(occupied)
}

fn normalize_item_stack(item: &mut HashMap<String, Value>) {
    let count = item
        .get("count")
        .and_then(value_to_i32)
        .or_else(|| item.get("Count").and_then(value_to_i32))
        .unwrap_or(1)
        .max(1);

    let count_byte = count.clamp(1, i8::MAX as i32) as i8;

    item.insert("count".to_string(), Value::Int(count));
    item.insert("Count".to_string(), Value::Byte(count_byte));

    if let Some(slot) = item.get("Slot").and_then(value_to_i32) {
        item.insert("Slot".to_string(), Value::Byte(slot.clamp(0, 127) as i8));
    }

    let Some(Value::String(id)) = item.get("id") else {
        return;
    };

    match id.as_str() {
        "minecraft:enchanted_book" => ensure_legacy_enchanted_book(item),
        "minecraft:writable_book" => ensure_legacy_writable_book(item),
        _ => {}
    }
}

fn ensure_legacy_enchanted_book(item: &mut HashMap<String, Value>) {
    let Some(Value::Compound(components)) = item.get("components") else {
        return;
    };

    let stored = match components.get("minecraft:stored_enchantments") {
        Some(Value::Compound(map)) => {
            let mut list = Vec::new();
            for (id, lvl) in map {
                let Some(level) = value_to_i32(lvl) else {
                    continue;
                };
                let mut entry = HashMap::new();
                entry.insert("id".to_string(), Value::String(id.clone()));
                entry.insert(
                    "lvl".to_string(),
                    Value::Short(level.clamp(0, i16::MAX as i32) as i16),
                );
                list.push(Value::Compound(entry));
            }
            list
        }
        Some(Value::List(list)) => {
            let mut entries = Vec::new();
            for value in list {
                if let Value::Compound(map) = value {
                    let Some(Value::String(id)) = map.get("id") else {
                        continue;
                    };
                    let Some(level) = map.get("lvl").and_then(value_to_i32) else {
                        continue;
                    };
                    let mut entry = HashMap::new();
                    entry.insert("id".to_string(), Value::String(id.clone()));
                    entry.insert(
                        "lvl".to_string(),
                        Value::Short(level.clamp(0, i16::MAX as i32) as i16),
                    );
                    entries.push(Value::Compound(entry));
                }
            }
            entries
        }
        _ => Vec::new(),
    };

    if stored.is_empty() {
        return;
    }

    let tag_entry = item
        .entry("tag".to_string())
        .or_insert_with(|| Value::Compound(HashMap::new()));

    if let Value::Compound(tag) = tag_entry {
        tag.entry("StoredEnchantments".to_string())
            .or_insert(Value::List(stored));
    }
}

fn ensure_legacy_writable_book(item: &mut HashMap<String, Value>) {
    let tag_entry = item
        .entry("tag".to_string())
        .or_insert_with(|| Value::Compound(HashMap::new()));

    if let Value::Compound(tag) = tag_entry {
        tag.entry("pages".to_string())
            .or_insert_with(|| Value::List(vec![Value::String(String::new())]));
    }
}

fn apply_bookshelf_slot_properties(props: &mut Option<Value>, occupied: [bool; 6]) {
    let mut map = match props.take() {
        Some(Value::Compound(map)) => map,
        _ => HashMap::new(),
    };

    for (idx, filled) in occupied.iter().enumerate() {
        map.insert(
            format!("slot_{}_occupied", idx),
            Value::String(if *filled { "true" } else { "false" }.to_string()),
        );
    }

    *props = Some(Value::Compound(map));
}

fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(v) => Some(i32::from(*v)),
        Value::Short(v) => Some(i32::from(*v)),
        Value::Int(v) => Some(*v),
        Value::Long(v) => i32::try_from(*v).ok(),
        Value::Float(v) => Some(*v as i32),
        Value::Double(v) => Some(*v as i32),
        _ => None,
    }
}

/// Generates interior layouts inside buildings at each floor level.
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
    is_abandoned_building: bool,
) {
    // Skip interior generation for very small buildings
    let width = max_x - min_x + 1;
    let depth = max_z - min_z + 1;

    if width < 8 || depth < 8 {
        return;
    }

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
        let mut wall_positions = HashSet::new();
        let mut door_positions = HashSet::new();

        let current_floor_ceiling = if floor_index < floor_levels.len() - 1 {
            floor_levels[floor_index + 1] - 1
        } else if args.roof
            && element.tags.contains_key("roof:shape")
            && element.tags.get("roof:shape").unwrap() != "flat"
        {
            start_y_offset + building_height
        } else {
            start_y_offset + building_height + 1
        };

        let template = if is_abandoned_building {
            if floor_index == 0 {
                get_template(&ABANDONED_INTERIOR_1_TEMPLATE, &ABANDONED_INTERIOR_1_CONFIG)
            } else {
                get_template(&ABANDONED_INTERIOR_2_TEMPLATE, &ABANDONED_INTERIOR_2_CONFIG)
            }
        } else if floor_index == 0 {
            get_template(&INTERIOR_1_TEMPLATE, &INTERIOR_1_CONFIG)
        } else {
            get_template(&INTERIOR_2_TEMPLATE, &INTERIOR_2_CONFIG)
        };

        let Some(template) = template else {
            continue;
        };

        place_template_for_floor(
            editor,
            template,
            &floor_area_set,
            interior_min_x,
            interior_min_z,
            interior_max_x,
            interior_max_z,
            floor_index,
            floor_y,
            abs_terrain_offset,
            wall_block,
            &mut wall_positions,
            &mut door_positions,
        );

        let extension_start = floor_y + INTERIOR_Y_OFFSET + template.template.size.1;
        if extension_start <= current_floor_ceiling {
            for (x, z) in &wall_positions {
                for y in extension_start..=current_floor_ceiling {
                    editor.set_block_absolute(
                        wall_block,
                        *x,
                        y + abs_terrain_offset,
                        *z,
                        None,
                        None,
                    );
                }
            }

            for (x, z) in &door_positions {
                for y in extension_start..=current_floor_ceiling {
                    editor.set_block_absolute(
                        wall_block,
                        *x,
                        y + abs_terrain_offset,
                        *z,
                        None,
                        None,
                    );
                }
            }
        }
    }
}
