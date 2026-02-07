use fastnbt::Value;
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub name: String,
    pub properties: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct TemplateBlock {
    pub pos: (i32, i32, i32),
    pub state: usize,
    pub nbt: Option<HashMap<String, Value>>,
}

#[derive(Clone, Debug)]
pub struct TemplateEntity {
    pub pos: (f64, f64, f64),
    pub block_pos: (i32, i32, i32),
    pub nbt: HashMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct StructureTemplate {
    pub size: (i32, i32, i32),
    pub palette: Vec<PaletteEntry>,
    pub blocks: Vec<TemplateBlock>,
    pub entities: Vec<TemplateEntity>,
}

impl StructureTemplate {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = resolve_structure_path(path.as_ref());
        let bytes = fs::read(&path)
            .map_err(|e| format!("Failed to read structure NBT file {}: {e}", path.display()))?;
        Self::from_bytes(&bytes)
            .map_err(|e| format!("Failed to parse structure NBT file {}: {e}", path.display()))
    }

    pub fn from_file_or_bytes(path: impl AsRef<Path>, fallback: &[u8]) -> Result<Self, String> {
        let path = resolve_structure_path(path.as_ref());
        if path.exists() {
            return Self::from_file(&path);
        }

        Self::from_bytes(fallback).map_err(|e| {
            format!(
                "Failed to parse embedded structure NBT {}: {e}",
                path.display()
            )
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let data = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = GzDecoder::new(bytes);
            let mut decompressed = Vec::new();
            decoder
                .read_to_end(&mut decompressed)
                .map_err(|e| format!("Failed to decompress gzip NBT data: {e}"))?;
            decompressed
        } else {
            bytes.to_vec()
        };

        let root: Value =
            fastnbt::from_bytes(&data).map_err(|e| format!("Failed to parse NBT payload: {e}"))?;
        let Value::Compound(root) = root else {
            return Err("Root tag is not a compound".to_string());
        };

        let size_value = root
            .get("size")
            .ok_or_else(|| "Missing size field".to_string())?;
        let size = parse_vec3_i32(size_value).ok_or_else(|| "Invalid size field".to_string())?;

        let palette = if let Some(palette_value) = root.get("palette") {
            parse_palette_list(palette_value)?
        } else if let Some(palettes_value) = root.get("palettes") {
            parse_palettes(palettes_value)?
        } else {
            return Err("Missing palette(s) field".to_string());
        };

        let blocks = if let Some(blocks_value) = root.get("blocks") {
            parse_blocks(blocks_value)?
        } else {
            Vec::new()
        };

        let entities = if let Some(entities_value) = root.get("entities") {
            parse_entities(entities_value)?
        } else {
            Vec::new()
        };

        Ok(Self {
            size,
            palette,
            blocks,
            entities,
        })
    }
}

fn resolve_structure_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    if path.exists() {
        return path.to_path_buf();
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = manifest_dir.join(path);
    if manifest_path.exists() {
        return manifest_path;
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let exe_relative = exe_dir.join(path);
            if exe_relative.exists() {
                return exe_relative;
            }
        }
    }

    path.to_path_buf()
}

fn parse_palette_list(value: &Value) -> Result<Vec<PaletteEntry>, String> {
    let Value::List(entries) = value else {
        return Err("Palette is not a list".to_string());
    };

    let mut palette = Vec::new();
    for entry in entries {
        let Value::Compound(map) = entry else {
            continue;
        };

        let name = match map.get("Name") {
            Some(Value::String(name)) => name.clone(),
            _ => continue,
        };

        let properties = match map.get("Properties") {
            Some(Value::Compound(props)) => Some(Value::Compound(props.clone())),
            _ => None,
        };

        palette.push(PaletteEntry { name, properties });
    }

    if palette.is_empty() {
        return Err("Palette is empty".to_string());
    }

    Ok(palette)
}

fn parse_palettes(value: &Value) -> Result<Vec<PaletteEntry>, String> {
    let Value::List(palettes) = value else {
        return Err("Palettes is not a list".to_string());
    };

    let first_palette = palettes
        .first()
        .ok_or_else(|| "Palettes list is empty".to_string())?;

    parse_palette_list(first_palette)
}

fn parse_blocks(value: &Value) -> Result<Vec<TemplateBlock>, String> {
    let Value::List(entries) = value else {
        return Err("Blocks is not a list".to_string());
    };

    let mut blocks = Vec::new();
    for entry in entries {
        let Value::Compound(map) = entry else {
            continue;
        };

        let pos_value = match map.get("pos") {
            Some(value) => value,
            None => continue,
        };

        let pos = match parse_vec3_i32(pos_value) {
            Some(pos) => pos,
            None => continue,
        };

        let state = match map.get("state").and_then(value_to_i32) {
            Some(state) if state >= 0 => state as usize,
            _ => continue,
        };

        let nbt = match map.get("nbt") {
            Some(Value::Compound(nbt)) => Some(nbt.clone()),
            _ => None,
        };

        blocks.push(TemplateBlock { pos, state, nbt });
    }

    Ok(blocks)
}

fn parse_entities(value: &Value) -> Result<Vec<TemplateEntity>, String> {
    let Value::List(entries) = value else {
        return Err("Entities is not a list".to_string());
    };

    let mut entities = Vec::new();
    for entry in entries {
        let Value::Compound(map) = entry else {
            continue;
        };

        let pos = match map.get("pos").and_then(parse_vec3_f64) {
            Some(pos) => pos,
            None => continue,
        };

        let block_pos = match map.get("blockPos").and_then(parse_vec3_i32) {
            Some(pos) => pos,
            None => continue,
        };

        let nbt = match map.get("nbt") {
            Some(Value::Compound(nbt)) => nbt.clone(),
            _ => continue,
        };

        entities.push(TemplateEntity {
            pos,
            block_pos,
            nbt,
        });
    }

    Ok(entities)
}

fn parse_vec3_i32(value: &Value) -> Option<(i32, i32, i32)> {
    match value {
        Value::List(values) => {
            if values.len() != 3 {
                return None;
            }
            Some((
                value_to_i32(&values[0])?,
                value_to_i32(&values[1])?,
                value_to_i32(&values[2])?,
            ))
        }
        Value::IntArray(values) => {
            if values.len() != 3 {
                return None;
            }
            Some((values[0], values[1], values[2]))
        }
        _ => None,
    }
}

fn parse_vec3_f64(value: &Value) -> Option<(f64, f64, f64)> {
    match value {
        Value::List(values) => {
            if values.len() != 3 {
                return None;
            }
            Some((
                value_to_f64(&values[0])?,
                value_to_f64(&values[1])?,
                value_to_f64(&values[2])?,
            ))
        }
        _ => None,
    }
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

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Byte(v) => Some(f64::from(*v)),
        Value::Short(v) => Some(f64::from(*v)),
        Value::Int(v) => Some(f64::from(*v)),
        Value::Long(v) => Some(*v as f64),
        Value::Float(v) => Some(f64::from(*v)),
        Value::Double(v) => Some(*v),
        _ => None,
    }
}
