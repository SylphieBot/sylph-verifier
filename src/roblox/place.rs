use byteorder::*;
use errors::*;
use roblox::lz4;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{Display, Formatter, Write as FmtWrite, Result as FmtResult};
use std::io::{Read, Write, Cursor};
use uuid::Uuid;

const RBLX_HEADER: &[u8] = b"<roblox!\x89\xff\r\n\x1a\n\x00\x00";
const RBLX_END: &[u8] = b"\x00\x00\x00\x00\t\x00\x00\x00\x00\x00\x00\x00</roblox>";

const INST_HEADER: u32 = 0x494E5354;
const PROP_HEADER: u32 = 0x50524F50;
const END0_HEADER: u32 = 0x454E4400;

const STRING_TYPE: u8 = 0x1;

#[derive(Clone, Debug)]
enum RblxCompressed<'a> {
    Compressed { decompressed_len: u32, data: &'a [u8] },
    Decompressed(Vec<u8>),
}
impl <'a> RblxCompressed<'a> {
    fn decompress(&'a self) -> Result<Cow<'a, [u8]>> {
        match *self {
            RblxCompressed::Compressed { decompressed_len, data } => {
                Ok(Cow::from(lz4::decompress(data, decompressed_len as usize)?))
            }
            RblxCompressed::Decompressed(ref vec) => {
                Ok(Cow::from(vec))
            }
        }
    }
    fn compress(&'a self) -> Result<(u32, Cow<'a, [u8]>)> {
        match *self {
            RblxCompressed::Compressed { decompressed_len, data } => {
                Ok((decompressed_len, Cow::from(data)))
            }
            RblxCompressed::Decompressed(ref vec) => {
                Ok((vec.len() as u32, Cow::from(lz4::compress(vec.as_ref())?)))
            }
        }
    }
}

#[derive(Clone, Debug)]
struct RblxEntry<'a> {
    kind: u32, data: RblxCompressed<'a>,
}

#[derive(Clone, Debug)]
struct RblxData<'a> {
    type_count: u32, inst_count: u32, entries: Vec<RblxEntry<'a>>,
}

fn read_cursor_slice<'a>(cursor: &mut Cursor<&'a [u8]>, len: usize) -> &'a [u8] {
    let start = cursor.position() as usize;
    let end   = start + len;
    let slice = &cursor.get_ref()[start..end];
    cursor.set_position(end as u64);
    slice
}
fn read_cursor_slice_to_end(cursor: Cursor<&[u8]>) -> &[u8] {
    let start = cursor.position() as usize;
    &cursor.into_inner()[start..]
}

fn check_unknown_field(mut r: impl Read) -> Result<()> {
    ensure!(r.read_u32::<LE>()? == 0, "unknown field has unexpected value");
    Ok(())
}
fn parse_rblx_container(data: &[u8]) -> Result<RblxData> {
    let mut cursor = Cursor::new(data);

    let mut read_header = [0u8; 16];
    cursor.read_exact(&mut read_header)?;
    ensure!(read_header == RBLX_HEADER, "incorrect place file header");

    let type_count = cursor.read_u32::<LE>()?;
    let inst_count = cursor.read_u32::<LE>()?;
    check_unknown_field(&mut cursor)?;
    check_unknown_field(&mut cursor)?;

    let mut entries = Vec::new();
    loop {
        let kind = cursor.read_u32::<BE>()?;
        if kind != END0_HEADER {
            let compressed_len = cursor.read_u32::<LE>()?;
            let decompressed_len = cursor.read_u32::<LE>()?;
            check_unknown_field(&mut cursor)?;
            let data = read_cursor_slice(&mut cursor, compressed_len as usize);
            entries.push(RblxEntry { kind, data: RblxCompressed::Compressed {
                decompressed_len, data
            }})
        } else {
            break
        }
    }

    ensure!(read_cursor_slice_to_end(cursor) == RBLX_END, "incorrect place file footer");
    Ok(RblxData { type_count, inst_count, entries })
}
fn parse_types(rblx: &RblxData) -> Result<HashMap<u32, String>> {
    let mut map = HashMap::new();
    for entry in &rblx.entries {
        if entry.kind == INST_HEADER {
            let data = entry.data.decompress()?;
            let mut cursor = Cursor::new(data.as_ref());

            let id = cursor.read_u32::<LE>()?;
            let name_len = cursor.read_u32::<LE>()?;
            let name = read_cursor_slice(&mut cursor, name_len as usize);

            map.insert(id, String::from_utf8(name.to_owned())?);
        }
    }
    Ok(map)
}

#[derive(Debug)]
struct RblxStringProperties {
    type_id: u32, prop_name: String, prop_values: Vec<String>
}
fn parse_string_property(data: &[u8]) -> Result<Option<RblxStringProperties>> {
    let mut cursor = Cursor::new(data);

    let type_id = cursor.read_u32::<LE>()?;
    let name_len = cursor.read_u32::<LE>()?;
    let name_slice = read_cursor_slice(&mut cursor, name_len as usize);
    let data_type = cursor.read_u8()?;

    if data_type == STRING_TYPE {
        let mut prop_values = Vec::new();
        while cursor.position() != data.len() as u64 {
            let data_len = cursor.read_u32::<LE>()?;
            let data_vec = read_cursor_slice(&mut cursor, data_len as usize).to_owned();
            match String::from_utf8(data_vec) {
                Ok(data) => { prop_values.push(data); }
                Err(_) => return Ok(None),
            }
        }
        Ok(Some(RblxStringProperties {
            type_id, prop_name: String::from_utf8(name_slice.to_owned())?, prop_values
        }))
    } else {
        Ok(None)
    }
}
fn write_string_property(mut w: impl Write, props: &RblxStringProperties) -> Result<()> {
    w.write_u32::<LE>(props.type_id)?;
    w.write_u32::<LE>(props.prop_name.len() as u32)?;
    w.write_all(props.prop_name.as_bytes())?;
    w.write_u8(STRING_TYPE)?;

    for str in &props.prop_values {
        w.write_u32::<LE>(str.len() as u32)?;
        w.write_all(str.as_bytes())?;
    }

    Ok(())
}
fn map_string_properties(
    rblx: &mut RblxData, mut f: impl FnMut(&str, &str, &str, &str) -> Result<Option<String>>
) -> Result<()> {

    let mut type_names = HashMap::new();
    let mut map_targets = Vec::new();
    {
        let mut types = parse_types(rblx)?;
        for entry in &mut rblx.entries {
            if entry.kind == PROP_HEADER {
                let prop = parse_string_property(entry.data.decompress()?.as_ref())?;
                if let Some(prop) = prop {
                    if prop.prop_name == "Name" {
                        let type_name = types.remove(&prop.type_id)?;
                        type_names.insert(prop.type_id, (type_name, Some(prop.prop_values)));
                    } else {
                        map_targets.push((prop.type_id, prop.prop_name, prop.prop_values, entry));
                    }
                }
            }
        }
        for (id, type_name) in types {
            type_names.insert(id, (type_name, None));
        }
    }
    for (type_id, prop_name, mut prop_values, entry_target) in map_targets {
        let mut modified = false;
        let type_data = type_names.get(&type_id)?;
        if let Some(ref names) = type_data.1 {
            for (value, name) in prop_values.iter_mut().zip(names.iter()) {
                if let Some(new_value) = f(&type_data.0, name, &prop_name, value)? {
                    modified = true;
                    *value = new_value;
                }
            }
            if modified {
                let new_property = RblxStringProperties { type_id, prop_name, prop_values };
                let mut cursor = Cursor::new(Vec::new());
                write_string_property(&mut cursor, &new_property)?;
                entry_target.data = RblxCompressed::Decompressed(cursor.into_inner());
            }
        }
    }
    Ok(())
}
fn write_rblx_container(mut w: impl Write, rblx: &RblxData) -> Result<()> {
    w.write_all(RBLX_HEADER)?;

    w.write_u32::<LE>(rblx.type_count)?;
    w.write_u32::<LE>(rblx.inst_count)?;
    w.write_u32::<LE>(0)?;
    w.write_u32::<LE>(0)?;

    for entry in &rblx.entries {
        w.write_u32::<BE>(entry.kind)?;
        let (uncompressed_size, data) = entry.data.compress()?;
        w.write_u32::<LE>(data.len() as u32)?;
        w.write_u32::<LE>(uncompressed_size)?;
        w.write_u32::<LE>(0)?;
        w.write_all(data.as_ref())?;
    }

    w.write_u32::<BE>(END0_HEADER)?;
    w.write_all(RBLX_END)?;

    Ok(())
}

#[derive(Clone, Debug)]
pub enum LuaConfigValue<'a> {
    Binary(Cow<'a, [u8]>), String(Cow<'a, str>), Double(f64), Nil,
}
impl <'a> Display for LuaConfigValue<'a> {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        match self {
            LuaConfigValue::Binary(ref b) => {
                write!(f, "\"")?;
                for byte in b.iter() {
                    write!(f, "\\{}", byte)?;
                }
                write!(f, "\"")?;
                Ok(())
            }
            LuaConfigValue::String(ref s) => write!(f, "[[{}]]", s.replace("]", "]]..']'..[[")),
            LuaConfigValue::Double(ref val) => val.fmt(f),
            LuaConfigValue::Nil => f.write_str("nil"),
        }
    }
}
impl <'a> From<&'a str> for LuaConfigValue<'a> {
    fn from(s: &'a str) -> Self {
        LuaConfigValue::String(Cow::from(s))
    }
}
impl <'a> From<&'a String> for LuaConfigValue<'a> {
    fn from(s: &'a String) -> Self {
        LuaConfigValue::String(Cow::from(s))
    }
}
impl <'a> From<String> for LuaConfigValue<'a> {
    fn from(s: String) -> Self {
        LuaConfigValue::String(Cow::from(s))
    }
}
impl <'a> From<&'a [u8]> for LuaConfigValue<'a> {
    fn from(b: &'a [u8]) -> Self {
        LuaConfigValue::Binary(Cow::from(b))
    }
}
impl <'a> From<&'a Vec<u8>> for LuaConfigValue<'a> {
    fn from(b: &'a Vec<u8>) -> Self {
        LuaConfigValue::Binary(Cow::from(b))
    }
}
impl <'a> From<Vec<u8>> for LuaConfigValue<'a> {
    fn from(b: Vec<u8>) -> Self {
        LuaConfigValue::Binary(Cow::from(b))
    }
}
impl <'a> From<i8> for LuaConfigValue<'a> {
    fn from(i: i8) -> Self {
        LuaConfigValue::Double(i as f64)
    }
}
impl <'a> From<u8> for LuaConfigValue<'a> {
    fn from(i: u8) -> Self {
        LuaConfigValue::Double(i as f64)
    }
}
impl <'a> From<i16> for LuaConfigValue<'a> {
    fn from(i: i16) -> Self {
        LuaConfigValue::Double(i as f64)
    }
}
impl <'a> From<u16> for LuaConfigValue<'a> {
    fn from(i: u16) -> Self {
        LuaConfigValue::Double(i as f64)
    }
}
impl <'a> From<i32> for LuaConfigValue<'a> {
    fn from(i: i32) -> Self {
        LuaConfigValue::Double(i as f64)
    }
}
impl <'a> From<u32> for LuaConfigValue<'a> {
    fn from(i: u32) -> Self {
        LuaConfigValue::Double(i as f64)
    }
}
// i64/u64 cannot be expressed unambigiously as f64
impl <'a> From<f64> for LuaConfigValue<'a> {
    fn from(d: f64) -> Self {
        LuaConfigValue::Double(d)
    }
}
impl <'a, T : Into<LuaConfigValue<'a>>> From<Option<T>> for LuaConfigValue<'a> {
    fn from(o: Option<T>) -> Self {
        match o {
            Some(x) => x.into(),
            None => LuaConfigValue::Nil,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LuaConfigEntry<'a> {
    name: &'static str, is_secret: bool, value: LuaConfigValue<'a>,
}
impl <'a> LuaConfigEntry<'a> {
    pub fn new(name: &'static str, is_secret: bool, v: impl Into<LuaConfigValue<'a>>) -> Self {
        LuaConfigEntry {
            name, is_secret, value: v.into(),
        }
    }
}

fn make_config(config: &[LuaConfigEntry], is_server: bool) -> Result<String> {
    let mut s = String::new();

    writeln!(s, "-- !!! DO NOT EDIT !!! --")?;
    writeln!(s, "--")?;
    writeln!(s, "-- Script automatically generated by Sylph-Verifier")?;
    writeln!(s, "-- Instead of editing this file, change the Sylph-Verifier configuration \
                    and regenerate the place file.")?;
    writeln!(s)?;
    if is_server {
        writeln!(s, "-- This script contains server secrets. Do not share it, or the place file \
                        that contains it.")?;
        writeln!(s)?;
    }
    writeln!(s, "local config = {{}}")?;
    for &LuaConfigEntry { name, is_secret, ref value } in config {
        if !is_secret || is_server {
            writeln!(s, "config.{} = {}", name, value)?;
        } else {
            writeln!(s, "-- Secret value {} omitted.", name)?;
        }
    }
    writeln!(s, "return config")?;

    Ok(s)
}

const PLACE_TEMPLATE: &[u8] = include_bytes!("place-template.rbxl");
const TEMPLATE_VERSION: &str = "1";
lazy_static! {
    static ref CONFIG_UUID_NAMESPACE: Uuid =
        "5314b09e-e38b-11e7-952b-5ef6654dc049".parse().unwrap();
}

pub fn create_place_file(overwrite_template: Option<&[u8]>,
                         config: &[LuaConfigEntry]) -> Result<Vec<u8>> {
    let place_file = overwrite_template.unwrap_or(PLACE_TEMPLATE);
    let mut place = parse_rblx_container(place_file)?;
    let mut version_found = false;

    let mut server_secure_config_source = false;
    let mut client_secure_config_source = false;

    let mut server_secure_config_uuid = false;
    let mut client_secure_config_uuid = false;

    let mut template_message = false;

    let server_config = make_config(config, true )?;
    let client_config = make_config(config, false)?;
    map_string_properties(&mut place, |type_name, obj_name, prop_name, prop_value| {
        Ok(match (type_name, obj_name, prop_name) {
            ("ModuleScript", "server_secure_config", "Source") => {
                trace!("Injecting server configuration.");
                server_secure_config_source = true;
                Some(server_config.clone())
            }
            ("ModuleScript", "client_config", "Source") => {
                trace!("Injecting client configuration.");
                client_secure_config_source = true;
                Some(client_config.clone())
            }
            ("ModuleScript", "server_secure_config", "ScriptGuid") => {
                trace!("Changing server configuration UUID.");
                server_secure_config_uuid = true;
                Some(format!("{{{}}}", Uuid::new_v5(&CONFIG_UUID_NAMESPACE, &server_config)))
            }
            ("ModuleScript", "client_config", "ScriptGuid") => {
                trace!("Changing client configuration UUID.");
                client_secure_config_uuid = true;
                Some(format!("{{{}}}", Uuid::new_v5(&CONFIG_UUID_NAMESPACE, &client_config)))
            }
            ("TextLabel", "TemplateMessage", "Text") => {
                trace!("Removing template message.");
                template_message = true;
                Some("".to_owned())
            }
            ("StringValue", "TemplateVersion", "Value") => {
                ensure!(prop_value == TEMPLATE_VERSION,
                        "wrong place template version: expected '{}', got '{}'",
                        TEMPLATE_VERSION, prop_value);
                trace!("TemplateVersion OK");
                version_found = true;
                None
            }
            _ => None,
        })
    })?;

    ensure!(server_secure_config_source && server_secure_config_uuid,
            "Place has no server config ModuleScript!");
    ensure!(client_secure_config_source && client_secure_config_uuid,
            "Place has no client config ModuleScript!");
    ensure!(template_message, "Place has no template marker TextLabel!");
    ensure!(version_found, "Place has no version property!");

    let mut cursor = Cursor::new(Vec::new());
    write_rblx_container(&mut cursor, &place)?;
    Ok(cursor.into_inner())
}