mod lz4;
mod place;

pub use self::place::{create_place_file, LuaConfigEntry, LuaConfigValue};

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RobloxUser {
    pub id: RobloxUserID, pub name: String,
}

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RobloxUserID(pub u64);