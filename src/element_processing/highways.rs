use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;
use std::collections::HashMap;

/// Type alias for highway connectivity map
pub type HighwayConnectivityMap = HashMap<(i32, i32), Vec<i32>>;

/// Generates highways with elevation support based on layer tags and connectivity analysis
pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
) {
    generate_highways_internal(editor, element, args, highway_connectivity);
}

/// Build a connectivity map for highway endpoints to determine where slopes are needed.
pub fn build_highway_connectivity_map(elements: &[ProcessedElement]) -> HighwayConnectivityMap {
    let mut connectivity_map: HashMap<(i32, i32), Vec<i32>> = HashMap::new();

    for element in elements {
        if let ProcessedElement::Way(way) = element {
            if way.tags.contains_key("highway") {
                let layer_value = way
                    .tags
                    .get("layer")
                    .and_then(|layer| layer.parse::<i32>().ok())
                    .unwrap_or(0);

                // Treat negative layers as ground level (0) for connectivity
                let layer_value = if layer_value < 0 { 0 } else { layer_value };

                // Add connectivity for start and end nodes
                if !way.nodes.is_empty() {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];

                    let start_coord = (start_node.x, start_node.z);
                    let end_coord = (end_node.x, end_node.z);

                    connectivity_map
                        .entry(start_coord)
                        .or_default()
                        .push(layer_value);
                    connectivity_map
                        .entry(end_coord)
                        .or_default()
                        .push(layer_value);
                }
            }
        }
    }

    connectivity_map
}

/// Internal function that generates highways with connectivity context for elevation handling
fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>, // Maps node coordinates to list of layers that connect to this node
) {
    if let Some(highway_type) = element.tags().get("highway") {
        if highway_type == "street_lamp" {
            // Handle street lamps
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                place_street_lamp(editor, x, 1, z);
            }
        } else if highway_type == "crossing" {
            // Handle traffic signals for crossings
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x: i32 = node.x;
                        let z: i32 = node.z;

                        for dy in 1..=3 {
                            editor.set_block(COBBLESTONE_WALL, x, dy, z, None, None);
                        }

                        editor.set_block(GREEN_WOOL, x, 4, z, None, None);
                        editor.set_block(YELLOW_WOOL, x, 5, z, None, None);
                        editor.set_block(RED_WOOL, x, 6, z, None, None);
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            // Handle bus stops
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;
                for dy in 1..=3 {
                    editor.set_block(COBBLESTONE_WALL, x, dy, z, None, None);
                }

                editor.set_block(WHITE_WOOL, x, 4, z, None, None);
                editor.set_block(WHITE_WOOL, x + 1, 4, z, None, None);
            }
        } else if element
            .tags()
            .get("area")
            .is_some_and(|v: &String| v == "yes")
        {
            let ProcessedElement::Way(way) = element else {
                return;
            };

            // Handle areas like pedestrian plazas
            let mut surface_block: Block = STONE; // Default block

            // Determine the block type based on the 'surface' tag
            if let Some(surface) = element.tags().get("surface") {
                surface_block = match surface.as_str() {
                    "paving_stones" | "sett" => STONE_BRICKS,
                    "bricks" => BRICK,
                    "wood" => OAK_PLANKS,
                    "asphalt" => BLACK_CONCRETE,
                    "gravel" | "fine_gravel" => GRAVEL,
                    "grass" => GRASS_BLOCK,
                    "dirt" | "ground" | "earth" => DIRT,
                    "sand" => SAND,
                    "concrete" => LIGHT_GRAY_CONCRETE,
                    _ => STONE, // Default to stone for unknown surfaces
                };
            }

            // Fill the area using flood fill or by iterating through the nodes
            let polygon_coords: Vec<(i32, i32)> = way
                .nodes
                .iter()
                .map(|n: &crate::osm_parser::ProcessedNode| (n.x, n.z))
                .collect();
            let filled_area: Vec<(i32, i32)> =
                flood_fill_area(&polygon_coords, args.timeout.as_ref());

            for (x, z) in filled_area {
                editor.set_block(surface_block, x, 0, z, None, None);
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            let mut block_type = BLACK_CONCRETE;
            let mut block_range: i32 = 2;
            let mut add_stripe = false;
            let mut add_outline = false;
            let scale_factor = args.scale;

            let building_passage_radius_m = element.tags().get("tunnel").and_then(|tunnel| {
                if tunnel == "building_passage" {
                    Some(determine_building_passage_radius(element.tags()))
                } else {
                    None
                }
            });

            // Parse the layer value for elevation calculation
            let layer_value = element
                .tags()
                .get("layer")
                .and_then(|layer| layer.parse::<i32>().ok())
                .unwrap_or(0);

            // Treat negative layers as ground level (0)
            let layer_value = if layer_value < 0 { 0 } else { layer_value };

            // Skip if 'level' is negative in the tags (indoor mapping)
            if let Some(level) = element.tags().get("level") {
                if level.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

            // Determine block type and range based on highway type
            match highway_type.as_str() {
                "footway" | "pedestrian" => {
                    block_type = GRAY_CONCRETE;
                    block_range = 1;
                }
                "path" => {
                    block_type = DIRT_PATH;
                    block_range = 1;
                }
                "motorway" | "primary" | "trunk" => {
                    block_range = 5;
                    add_stripe = true;
                }
                "secondary" => {
                    block_range = 4;
                    add_stripe = true;
                }
                "tertiary" => {
                    add_stripe = true;
                }
                "track" => {
                    block_range = 1;
                }
                "service" => {
                    block_type = GRAY_CONCRETE;
                    block_range = 2;
                }
                "secondary_link" | "tertiary_link" => {
                    //Exit ramps, sliproads
                    block_type = BLACK_CONCRETE;
                    block_range = 1;
                }
                "escape" => {
                    // Sand trap for vehicles on mountainous roads
                    block_type = SAND;
                    block_range = 1;
                }
                "steps" => {
                    //TODO: Add correct stairs respecting height, step_count, etc.
                    block_type = GRAY_CONCRETE;
                    block_range = 1;
                }

                _ => {
                    if let Some(lanes) = element.tags().get("lanes") {
                        if lanes == "2" {
                            block_range = 3;
                            add_stripe = true;
                            add_outline = true;
                        } else if lanes != "1" {
                            block_range = 4;
                            add_stripe = true;
                            add_outline = true;
                        }
                    }
                }
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

            if scale_factor < 1.0 {
                block_range = ((block_range as f64) * scale_factor).floor() as i32;
            }

            let parse_numeric_width = |value: Option<&String>| -> Option<f64> {
                value.and_then(|v| {
                    if v.chars().all(|c: char| c.is_ascii_digit() || c == '.') {
                        v.parse::<f64>().ok()
                    } else {
                        None
                    }
                })
            };

            let parsed_width_meters = element
                .tags()
                .get("width")
                .and_then(|value| parse_numeric_width(Some(value)));
            let lane_count = element
                .tags()
                .get("lanes")
                .and_then(|lanes: &String| lanes.parse::<f64>().ok());

            let parking_left = parsed_width_meters.is_none()
                && (matches!(
                    element.tags().get("parking:left"),
                    Some(v) if v == "lane"
                ) || matches!(
                    element.tags().get("parking:both"),
                    Some(v) if v == "lane"
                ));
            let parking_right = parsed_width_meters.is_none()
                && (matches!(
                    element.tags().get("parking:right"),
                    Some(v) if v == "lane"
                ) || matches!(
                    element.tags().get("parking:both"),
                    Some(v) if v == "lane"
                ));

            let parking_extra_left_m = if parking_left { 1.5 } else { 0.0 };
            let parking_extra_right_m = if parking_right { 1.5 } else { 0.0 };

            let base_width_m = if let Some(width) = parsed_width_meters {
                width
            } else if let Some(lanes) = lane_count {
                lanes * 3.0
            } else {
                (block_range as f64 * 2.0 + 1.0) / args.scale
            };
            let base_half_width_m = base_width_m / 2.0;

            let left_half_width_m = base_half_width_m + parking_extra_left_m;
            let right_half_width_m = base_half_width_m + parking_extra_right_m;

            let derived_range = ((base_half_width_m * args.scale).floor() as i32).max(0);
            let left_side_range = ((left_half_width_m * args.scale).floor() as i32).max(0);
            let right_side_range = ((right_half_width_m * args.scale).floor() as i32).max(0);

            let override_width = parsed_width_meters.is_some()
                || lane_count.is_some()
                || parking_left
                || parking_right;
            if override_width {
                block_range = derived_range;
            }

            block_range = block_range
                .max(left_side_range)
                .max(right_side_range)
                .max(derived_range);

            if let Some(lane_markings) = element.tags().get("lane_markings") {
                match lane_markings.as_str() {
                    "yes" => add_stripe = true,
                    "no" => add_stripe = false,
                    _ => {}
                }
            }

            let mut sidewalk_left = false;
            let mut sidewalk_right = false;
            if let Some(sidewalk) = element.tags().get("sidewalk") {
                match sidewalk.as_str() {
                    "both" | "yes" => {
                        sidewalk_left = true;
                        sidewalk_right = true;
                    }
                    "left" => sidewalk_left = true,
                    "right" => sidewalk_right = true,
                    _ => {}
                }
            }
            if matches!(
                element.tags().get("sidewalk:left"),
                Some(v) if v == "yes"
            ) || matches!(
                element.tags().get("sidewalk:both"),
                Some(v) if v == "yes"
            ) {
                sidewalk_left = true;
            }
            if matches!(
                element.tags().get("sidewalk:right"),
                Some(v) if v == "yes"
            ) || matches!(
                element.tags().get("sidewalk:both"),
                Some(v) if v == "yes"
            ) {
                sidewalk_right = true;
            }

            let sidewalk_both_width_m =
                parse_numeric_width(element.tags().get("sidewalk:both:width"));
            let sidewalk_left_width_m =
                parse_numeric_width(element.tags().get("sidewalk:left:width"))
                    .or(sidewalk_both_width_m);
            let sidewalk_right_width_m =
                parse_numeric_width(element.tags().get("sidewalk:right:width"))
                    .or(sidewalk_both_width_m);

            let default_sidewalk_range = if scale_factor < 1.0 {
                ((1.0f64 * scale_factor).floor() as i32).max(0)
            } else {
                1
            };
            let default_sidewalk_width_blocks = (default_sidewalk_range * 2 + 1).max(1);
            let sidewalk_left_width_blocks = sidewalk_left_width_m
                .map(|w| width_blocks_from_meters(w, scale_factor))
                .unwrap_or(default_sidewalk_width_blocks);
            let sidewalk_right_width_blocks = sidewalk_right_width_m
                .map(|w| width_blocks_from_meters(w, scale_factor))
                .unwrap_or(default_sidewalk_width_blocks);
            let has_lighting = matches!(element.tags().get("lit"), Some(v) if v == "yes");
            let (lamp_left, lamp_right) = if has_lighting {
                determine_lamp_sides(element.tags(), highway_type.as_str())
            } else {
                (false, false)
            };
            let lamp_spacing_blocks: usize = ((16.0 * scale_factor).ceil() as usize).max(8);
            let mut lamp_step_counter: usize = 0;

            // Calculate elevation based on layer
            const LAYER_HEIGHT_STEP: i32 = 6; // Each layer is 6 blocks higher/lower
            let base_elevation = layer_value * LAYER_HEIGHT_STEP;

            // Check if we need slopes at start and end
            let needs_start_slope =
                should_add_slope_at_node(&way.nodes[0], layer_value, highway_connectivity);
            let needs_end_slope = should_add_slope_at_node(
                &way.nodes[way.nodes.len() - 1],
                layer_value,
                highway_connectivity,
            );

            // Calculate total way length for slope distribution
            let total_way_length = calculate_way_length(way);

            // Check if this is a short isolated elevated segment - if so, treat as ground level
            let is_short_isolated_elevated =
                needs_start_slope && needs_end_slope && layer_value > 0 && total_way_length <= 35;

            // Override elevation and slopes for short isolated segments
            let (effective_elevation, effective_start_slope, effective_end_slope) =
                if is_short_isolated_elevated {
                    (0, false, false) // Treat as ground level
                } else {
                    (base_elevation, needs_start_slope, needs_end_slope)
                };

            let slope_length = (total_way_length as f32 * 0.35).clamp(15.0, 50.0) as usize; // 35% of way length, max 50 blocks, min 15 blocks

            // Iterate over nodes to create the highway
            let mut segment_index = 0;
            let total_segments = way.nodes.len() - 1;

            for node in &way.nodes {
                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let x2: i32 = node.x;
                    let z2: i32 = node.z;

                    let segment_dx = x2 - x1;
                    let segment_dz = z2 - z1;

                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, 0, z1, x2, 0, z2);

                    let (x_range_neg, x_range_pos, z_range_neg, z_range_pos) =
                        calculate_segment_ranges(
                            x1,
                            z1,
                            x2,
                            z2,
                            block_range,
                            left_side_range,
                            right_side_range,
                        );

                    // Calculate elevation for this segment
                    let segment_length = bresenham_points.len();

                    // Variables to manage dashed line pattern
                    let mut stripe_length: i32 = 0;
                    let dash_length: i32 = (5.0 * scale_factor).ceil() as i32;
                    let gap_length: i32 = (5.0 * scale_factor).ceil() as i32;
                    let side_offsets = calculate_side_offsets(
                        x1,
                        z1,
                        x2,
                        z2,
                        x_range_neg,
                        x_range_pos,
                        z_range_neg,
                        z_range_pos,
                        sidewalk_left,
                        sidewalk_right,
                        sidewalk_left_width_blocks,
                        sidewalk_right_width_blocks,
                        add_outline,
                    );

                    for (point_index, &(x, _, z)) in bresenham_points.iter().enumerate() {
                        // Calculate Y elevation for this point based on slopes and layer
                        let current_y = calculate_point_elevation(
                            segment_index,
                            point_index,
                            segment_length,
                            total_segments,
                            effective_elevation,
                            effective_start_slope,
                            effective_end_slope,
                            slope_length,
                        );

                        // Draw the road surface for the entire width
                        for dx_offset in -x_range_neg..=x_range_pos {
                            for dz_offset in -z_range_neg..=z_range_pos {
                                let set_x: i32 = x + dx_offset;
                                let set_z: i32 = z + dz_offset;

                                // Zebra crossing logic
                                if highway_type == "footway"
                                    && element.tags().get("footway")
                                        == Some(&"crossing".to_string())
                                {
                                    let is_horizontal: bool = (x2 - x1).abs() >= (z2 - z1).abs();
                                    if is_horizontal {
                                        if set_x % 2 < 1 {
                                            editor.set_block(
                                                WHITE_CONCRETE,
                                                set_x,
                                                current_y,
                                                set_z,
                                                Some(&[BLACK_CONCRETE]),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                BLACK_CONCRETE,
                                                set_x,
                                                current_y,
                                                set_z,
                                                None,
                                                None,
                                            );
                                        }
                                    } else if set_z % 2 < 1 {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            set_x,
                                            current_y,
                                            set_z,
                                            Some(&[BLACK_CONCRETE]),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            BLACK_CONCRETE,
                                            set_x,
                                            current_y,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }
                                } else {
                                    editor.set_block(
                                        block_type,
                                        set_x,
                                        current_y,
                                        set_z,
                                        None,
                                        Some(&[BLACK_CONCRETE, WHITE_CONCRETE]),
                                    );
                                }

                                // Add stone brick foundation underneath elevated highways for thickness
                                if effective_elevation > 0 && current_y > 0 {
                                    // Add 1 layer of stone bricks underneath the highway surface
                                    editor.set_block(
                                        STONE_BRICKS,
                                        set_x,
                                        current_y - 1,
                                        set_z,
                                        None,
                                        None,
                                    );
                                }

                                // Add support pillars for elevated highways
                                if effective_elevation != 0 && current_y > 0 {
                                    add_highway_support_pillar(
                                        editor,
                                        set_x,
                                        current_y,
                                        set_z,
                                        dx_offset,
                                        dz_offset,
                                        block_range,
                                    );
                                }
                            }
                        }

                        if sidewalk_left || sidewalk_right {
                            add_sidewalks(
                                editor,
                                x1,
                                z1,
                                x2,
                                z2,
                                x,
                                z,
                                current_y,
                                x_range_neg,
                                x_range_pos,
                                z_range_neg,
                                z_range_pos,
                                sidewalk_left,
                                sidewalk_right,
                                sidewalk_left_width_blocks,
                                sidewalk_right_width_blocks,
                            );
                        }

                        // Add light gray concrete outline for multi-lane roads
                        if add_outline {
                            if (x2 - x1).abs() >= (z2 - z1).abs() {
                                // Dominant direction along X, outline along Z edges
                                let outline_z_neg = z - z_range_neg - 1;
                                let outline_z_pos = z + z_range_pos + 1;

                                for dx_outline in -x_range_neg..=x_range_pos {
                                    let outline_x = x + dx_outline;
                                    editor.set_block(
                                        LIGHT_GRAY_CONCRETE,
                                        outline_x,
                                        current_y,
                                        outline_z_neg,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        LIGHT_GRAY_CONCRETE,
                                        outline_x,
                                        current_y,
                                        outline_z_pos,
                                        None,
                                        None,
                                    );
                                }
                            } else {
                                // Dominant direction along Z, outline along X edges
                                let outline_x_neg = x - x_range_neg - 1;
                                let outline_x_pos = x + x_range_pos + 1;

                                for dz_outline in -z_range_neg..=z_range_pos {
                                    let outline_z = z + dz_outline;
                                    editor.set_block(
                                        LIGHT_GRAY_CONCRETE,
                                        outline_x_neg,
                                        current_y,
                                        outline_z,
                                        None,
                                        None,
                                    );
                                    editor.set_block(
                                        LIGHT_GRAY_CONCRETE,
                                        outline_x_pos,
                                        current_y,
                                        outline_z,
                                        None,
                                        None,
                                    );
                                }
                            }
                        }

                        // Add a dashed white line in the middle for larger roads
                        if let Some(radius_m) = building_passage_radius_m {
                            generate_building_passage_tunnel_at_point(
                                editor, x, z, current_y, segment_dx, segment_dz, radius_m,
                                args.scale,
                            );
                        }

                        if add_stripe {
                            if stripe_length < dash_length {
                                let stripe_x: i32 = x;
                                let stripe_z: i32 = z;
                                editor.set_block(
                                    WHITE_CONCRETE,
                                    stripe_x,
                                    current_y,
                                    stripe_z,
                                    Some(&[BLACK_CONCRETE]),
                                    None,
                                );
                            }

                            // Increment stripe_length and reset after completing a dash and gap
                            stripe_length += 1;
                            if stripe_length >= dash_length + gap_length {
                                stripe_length = 0;
                            }
                        }

                        if has_lighting {
                            lamp_step_counter += 1;
                            if lamp_step_counter >= lamp_spacing_blocks {
                                if lamp_left {
                                    let (lamp_x, lamp_z) = side_offsets.left_position(x, z);
                                    place_street_lamp(editor, lamp_x, current_y + 1, lamp_z);
                                }

                                if lamp_right {
                                    let (lamp_x, lamp_z) = side_offsets.right_position(x, z);
                                    place_street_lamp(editor, lamp_x, current_y + 1, lamp_z);
                                }

                                lamp_step_counter = 0;
                            }
                        }
                    }

                    segment_index += 1;
                }
                previous_node = Some((node.x, node.z));
            }
        }
    }
}

/// Helper function to determine if a slope should be added at a specific node
fn should_add_slope_at_node(
    node: &crate::osm_parser::ProcessedNode,
    current_layer: i32,
    highway_connectivity: &HashMap<(i32, i32), Vec<i32>>,
) -> bool {
    let node_coord = (node.x, node.z);

    // If we don't have connectivity information, always add slopes for non-zero layers
    if highway_connectivity.is_empty() {
        return current_layer != 0;
    }

    // Check if there are other highways at different layers connected to this node
    if let Some(connected_layers) = highway_connectivity.get(&node_coord) {
        // Count how many ways are at the same layer as current way
        let same_layer_count = connected_layers
            .iter()
            .filter(|&&layer| layer == current_layer)
            .count();

        // If this is the only way at this layer connecting to this node, we need a slope
        // (unless we're at ground level and connecting to ground level ways)
        if same_layer_count <= 1 {
            return current_layer != 0;
        }

        // If there are multiple ways at the same layer, don't add slope
        false
    } else {
        // No other highways connected, add slope if not at ground level
        current_layer != 0
    }
}

/// Helper function to calculate the total length of a way in blocks
fn calculate_way_length(way: &ProcessedWay) -> usize {
    let mut total_length = 0;
    let mut previous_node: Option<&crate::osm_parser::ProcessedNode> = None;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let dx = (node.x - prev.x).abs();
            let dz = (node.z - prev.z).abs();
            total_length += ((dx * dx + dz * dz) as f32).sqrt() as usize;
        }
        previous_node = Some(node);
    }

    total_length
}

/// Calculate the Y elevation for a specific point along the highway
#[allow(clippy::too_many_arguments)]
fn calculate_point_elevation(
    segment_index: usize,
    point_index: usize,
    segment_length: usize,
    total_segments: usize,
    base_elevation: i32,
    needs_start_slope: bool,
    needs_end_slope: bool,
    slope_length: usize,
) -> i32 {
    // If no slopes needed, return base elevation
    if !needs_start_slope && !needs_end_slope {
        return base_elevation;
    }

    // Calculate total distance from start
    let total_distance_from_start = segment_index * segment_length + point_index;
    let total_way_length = total_segments * segment_length;

    // Ensure we have reasonable values
    if total_way_length == 0 || slope_length == 0 {
        return base_elevation;
    }

    // Start slope calculation - gradual rise from ground level
    if needs_start_slope && total_distance_from_start <= slope_length {
        let slope_progress = total_distance_from_start as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // End slope calculation - gradual descent to ground level
    if needs_end_slope
        && total_distance_from_start >= (total_way_length.saturating_sub(slope_length))
    {
        let distance_from_end = total_way_length - total_distance_from_start;
        let slope_progress = distance_from_end as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // Middle section at full elevation
    base_elevation
}

/// Calculate how many blocks to extend to each side for a segment, respecting asymmetrical widths
fn calculate_segment_ranges(
    x1: i32,
    z1: i32,
    x2: i32,
    z2: i32,
    base_range: i32,
    left_side_range: i32,
    right_side_range: i32,
) -> (i32, i32, i32, i32) {
    if (x2 - x1).abs() >= (z2 - z1).abs() {
        // Dominant axis is X, so left/right expand along Z
        let (z_neg, z_pos) = if x2 - x1 >= 0 {
            (right_side_range, left_side_range)
        } else {
            (left_side_range, right_side_range)
        };
        (base_range, base_range, z_neg, z_pos)
    } else {
        // Dominant axis is Z, so left/right expand along X
        let (x_neg, x_pos) = if z2 - z1 >= 0 {
            (left_side_range, right_side_range)
        } else {
            (right_side_range, left_side_range)
        };
        (x_neg, x_pos, base_range, base_range)
    }
}

fn determine_building_passage_radius(tags: &HashMap<String, String>) -> f64 {
    const DEFAULT_RADIUS: f64 = 5.0;
    parse_building_passage_value(tags.get("maxheight:physical"))
        .or_else(|| parse_building_passage_value(tags.get("height")))
        .or_else(|| parse_building_passage_value(tags.get("maxheight")))
        .unwrap_or(DEFAULT_RADIUS)
}

fn parse_building_passage_value(value: Option<&String>) -> Option<f64> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }

        match trimmed.to_lowercase().as_str() {
            "default" => Some(5.0),
            "below_default" => Some(3.0),
            _ => {
                let cleaned = trimmed
                    .trim_end_matches(|c: char| c == 'm' || c == 'M')
                    .trim();

                if cleaned.is_empty() {
                    return None;
                }

                if cleaned.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    cleaned.parse::<f64>().ok()
                } else {
                    None
                }
            }
        }
    })
}

fn generate_building_passage_tunnel_at_point(
    editor: &mut WorldEditor,
    center_x: i32,
    center_z: i32,
    base_y: i32,
    dir_dx: i32,
    dir_dz: i32,
    radius_m: f64,
    scale_factor: f64,
) {
    let radius_blocks = (radius_m * scale_factor).max(0.0);
    if radius_blocks <= 0.0 {
        return;
    }

    let radius_sq = radius_blocks * radius_blocks;
    let max_height_blocks = (radius_blocks.ceil() as i32).max(1);

    let mut dir_unit_x = dir_dx as f64;
    let mut dir_unit_z = dir_dz as f64;
    let len = (dir_unit_x * dir_unit_x + dir_unit_z * dir_unit_z).sqrt();
    if len > 0.0 {
        dir_unit_x /= len;
        dir_unit_z /= len;
    } else {
        dir_unit_x = 1.0;
        dir_unit_z = 0.0;
    }

    let perp_unit_x = -dir_unit_z;
    let perp_unit_z = dir_unit_x;
    let max_offset = (radius_blocks.ceil() as i32) + 2; // Extended to cover 2-block thick shell

    for height_offset in 1..=max_height_blocks {
        let height_center = (height_offset as f64) - 0.5;
        if height_center > radius_blocks + 2.5 {
            break;
        }

        let horizontal_limit_sq = (radius_sq - height_center * height_center).max(0.0);
        let horizontal_limit = horizontal_limit_sq.sqrt();
        let y = base_y + height_offset;

        for dx_offset in -max_offset..=max_offset {
            for dz_offset in -max_offset..=max_offset {
                let perp_distance =
                    (dx_offset as f64) * perp_unit_x + (dz_offset as f64) * perp_unit_z;
                let perp_abs = perp_distance.abs();

                // Extended range to include 2-block thick shell on the outside
                if perp_abs > horizontal_limit + 2.5 {
                    continue;
                }

                let parallel_distance =
                    (dx_offset as f64) * dir_unit_x + (dz_offset as f64) * dir_unit_z;
                
                // Interior is more than 2 blocks away from the edge and below the top
                let is_interior = perp_abs < (horizontal_limit - 0.5) && height_center < (radius_blocks - 1.5);

                // For STRUCTURE_VOID interior, extend further in direction
                if is_interior && parallel_distance.abs() <= horizontal_limit + 1.0 {
                    editor.set_block(
                        STRUCTURE_VOID,
                        center_x + dx_offset,
                        y,
                        center_z + dz_offset,
                        None,
                        Some(&[]),
                    );
                } else if !is_interior && parallel_distance.abs() <= 1.5 {
                    // COBBLED_DEEPSLATE shell stays in original radius
                    editor.set_block(
                        COBBLED_DEEPSLATE,
                        center_x + dx_offset,
                        y,
                        center_z + dz_offset,
                        None,
                        Some(&[]),
                    );
                }
            }
        }
    }
}

fn width_blocks_from_meters(width_meters: f64, scale_factor: f64) -> i32 {
    ((width_meters * scale_factor).ceil() as i32).max(1)
}

#[derive(Clone, Copy)]
enum SideAxis {
    X,
    Z,
}

#[derive(Clone, Copy)]
struct SideOffsets {
    left_axis: SideAxis,
    right_axis: SideAxis,
    left_sign: i32,
    right_sign: i32,
    left_offset: i32,
    right_offset: i32,
}

impl SideOffsets {
    fn left_position(&self, x: i32, z: i32) -> (i32, i32) {
        match self.left_axis {
            SideAxis::X => (x + self.left_sign * self.left_offset, z),
            SideAxis::Z => (x, z + self.left_sign * self.left_offset),
        }
    }

    fn right_position(&self, x: i32, z: i32) -> (i32, i32) {
        match self.right_axis {
            SideAxis::X => (x + self.right_sign * self.right_offset, z),
            SideAxis::Z => (x, z + self.right_sign * self.right_offset),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn calculate_side_offsets(
    x1: i32,
    z1: i32,
    x2: i32,
    z2: i32,
    x_range_neg: i32,
    x_range_pos: i32,
    z_range_neg: i32,
    z_range_pos: i32,
    sidewalk_left: bool,
    sidewalk_right: bool,
    sidewalk_left_width: i32,
    sidewalk_right_width: i32,
    add_outline: bool,
) -> SideOffsets {
    let margin = 1 + i32::from(add_outline);

    if (x2 - x1).abs() >= (z2 - z1).abs() {
        // Left/right expand along Z
        let (left_range, right_range, left_sign, right_sign) = if x2 - x1 >= 0 {
            (z_range_pos, z_range_neg, 1, -1)
        } else {
            (z_range_neg, z_range_pos, -1, 1)
        };

        let left_offset = (left_range
            + margin
            + if sidewalk_left {
                sidewalk_left_width
            } else {
                0
            })
        .max(1);
        let right_offset = (right_range
            + margin
            + if sidewalk_right {
                sidewalk_right_width
            } else {
                0
            })
        .max(1);

        SideOffsets {
            left_axis: SideAxis::Z,
            right_axis: SideAxis::Z,
            left_sign,
            right_sign,
            left_offset,
            right_offset,
        }
    } else {
        // Left/right expand along X
        let (left_range, right_range, left_sign, right_sign) = if z2 - z1 >= 0 {
            (x_range_neg, x_range_pos, -1, 1)
        } else {
            (x_range_pos, x_range_neg, 1, -1)
        };

        let left_offset = (left_range
            + margin
            + if sidewalk_left {
                sidewalk_left_width
            } else {
                0
            })
        .max(1);
        let right_offset = (right_range
            + margin
            + if sidewalk_right {
                sidewalk_right_width
            } else {
                0
            })
        .max(1);

        SideOffsets {
            left_axis: SideAxis::X,
            right_axis: SideAxis::X,
            left_sign,
            right_sign,
            left_offset,
            right_offset,
        }
    }
}

fn determine_lamp_sides(tags: &HashMap<String, String>, highway_type: &str) -> (bool, bool) {
    if let Some(side_value) = tags.get("lit:side") {
        match side_value.as_str() {
            "left" => return (true, false),
            "right" => return (false, true),
            "both" => return (true, true),
            _ => {}
        }
    }

    let large_road = matches!(
        highway_type,
        "primary"
            | "secondary"
            | "tertiary"
            | "trunk"
            | "motorway"
            | "primary_link"
            | "secondary_link"
            | "tertiary_link"
    );

    if large_road {
        return (true, true);
    }

    let mut rng = rand::thread_rng();
    if rng.gen_bool(0.5) {
        (true, false)
    } else {
        (false, true)
    }
}

fn place_street_lamp(editor: &mut WorldEditor, x: i32, base_y: i32, z: i32) {
    let absolute_base_y = editor.get_absolute_y(x, base_y, z);
    if has_nearby_lamp(editor, absolute_base_y, x, z, 10) {
        return;
    }

    editor.set_block(COBBLESTONE_WALL, x, base_y, z, None, None);
    for dy in 1..=3 {
        editor.set_block(OAK_FENCE, x, base_y + dy, z, None, None);
    }
    editor.set_block(GLOWSTONE, x, base_y + 4, z, None, None);
}

fn has_nearby_lamp(
    editor: &WorldEditor,
    absolute_base_y: i32,
    x: i32,
    z: i32,
    radius: i32,
) -> bool {
    let radius_sq = radius * radius;

    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx * dx + dz * dz > radius_sq {
                continue;
            }

            if editor.check_for_block_absolute(
                x + dx,
                absolute_base_y,
                z + dz,
                Some(&[COBBLESTONE_WALL]),
                None,
            ) {
                return true;
            }
        }
    }

    false
}

#[allow(clippy::too_many_arguments)]
fn add_sidewalks(
    editor: &mut WorldEditor,
    x1: i32,
    z1: i32,
    x2: i32,
    z2: i32,
    point_x: i32,
    point_z: i32,
    y: i32,
    x_range_neg: i32,
    x_range_pos: i32,
    z_range_neg: i32,
    z_range_pos: i32,
    sidewalk_left: bool,
    sidewalk_right: bool,
    sidewalk_left_width: i32,
    sidewalk_right_width: i32,
) {
    let block_type = GRAY_CONCRETE;
    if (x2 - x1).abs() >= (z2 - z1).abs() {
        let (left_range, right_range, left_sign, right_sign) = if x2 - x1 >= 0 {
            (z_range_pos, z_range_neg, 1, -1)
        } else {
            (z_range_neg, z_range_pos, -1, 1)
        };

        if sidewalk_left {
            let start = left_range + 1;
            let end = left_range + sidewalk_left_width;
            for offset in start..=end {
                let set_z = point_z + offset * left_sign;
                for dx in -x_range_neg..=x_range_pos {
                    editor.set_block(block_type, point_x + dx, y, set_z, None, None);
                }
            }
        }

        if sidewalk_right {
            let start = right_range + 1;
            let end = right_range + sidewalk_right_width;
            for offset in start..=end {
                let set_z = point_z + offset * right_sign;
                for dx in -x_range_neg..=x_range_pos {
                    editor.set_block(block_type, point_x + dx, y, set_z, None, None);
                }
            }
        }
    } else {
        let (left_range, right_range, left_sign, right_sign) = if z2 - z1 >= 0 {
            (x_range_neg, x_range_pos, -1, 1)
        } else {
            (x_range_pos, x_range_neg, 1, -1)
        };

        if sidewalk_left {
            let start = left_range + 1;
            let end = left_range + sidewalk_left_width;
            for offset in start..=end {
                let set_x = point_x + offset * left_sign;
                for dz in -z_range_neg..=z_range_pos {
                    editor.set_block(block_type, set_x, y, point_z + dz, None, None);
                }
            }
        }

        if sidewalk_right {
            let start = right_range + 1;
            let end = right_range + sidewalk_right_width;
            for offset in start..=end {
                let set_x = point_x + offset * right_sign;
                for dz in -z_range_neg..=z_range_pos {
                    editor.set_block(block_type, set_x, y, point_z + dz, None, None);
                }
            }
        }
    }
}

/// Add support pillars for elevated highways
fn add_highway_support_pillar(
    editor: &mut WorldEditor,
    x: i32,
    highway_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32, // Keep for future use
) {
    // Only add pillars at specific intervals and positions
    if dx == 0 && dz == 0 && (x + z) % 8 == 0 {
        // Add pillar from ground to highway level
        for y in 1..highway_y {
            editor.set_block(STONE_BRICKS, x, y, z, None, None);
        }

        // Add pillar base
        for base_dx in -1..=1 {
            for base_dz in -1..=1 {
                editor.set_block(STONE_BRICKS, x + base_dx, 0, z + base_dz, None, None);
            }
        }
    }
}

/// Generates a siding using stone brick slabs
pub fn generate_siding(editor: &mut WorldEditor, element: &ProcessedWay) {
    let mut previous_node: Option<XZPoint> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let current_node = node.xz();

        // Draw the siding using Bresenham's line algorithm between nodes
        if let Some(prev_node) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                if !editor.check_for_block(bx, 0, bz, Some(&[BLACK_CONCRETE, WHITE_CONCRETE])) {
                    editor.set_block(siding_block, bx, 1, bz, None, None);
                }
            }
        }

        previous_node = Some(current_node);
    }
}

/// Generates an aeroway
pub fn generate_aeroway(editor: &mut WorldEditor, way: &ProcessedWay, args: &Args) {
    let mut previous_node: Option<(i32, i32)> = None;
    let surface_block = LIGHT_GRAY_CONCRETE;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let (x1, z1) = prev;
            let x2 = node.x;
            let z2 = node.z;
            let points = bresenham_line(x1, 0, z1, x2, 0, z2);
            let way_width: i32 = (12.0 * args.scale).ceil() as i32;

            for (x, _, z) in points {
                for dx in -way_width..=way_width {
                    for dz in -way_width..=way_width {
                        let set_x = x + dx;
                        let set_z = z + dz;
                        editor.set_block(surface_block, set_x, 0, set_z, None, None);
                    }
                }
            }
        }
        previous_node = Some((node.x, node.z));
    }
}
