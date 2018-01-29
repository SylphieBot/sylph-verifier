mod api;
mod lz4;
mod place;
mod rules;

pub use self::place::{create_place_file, LuaConfigEntry, LuaConfigValue};
pub use self::rules::{VerificationRule, VerificationSet};

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RobloxUserID(pub u64);
impl RobloxUserID {
    pub fn for_username(name: &str) -> ::errors::Result<RobloxUserID> {
        match api::for_username(name)? {
            Some(id) => Ok(id),
            None => cmd_error!("No Roblox user named '{}' found.", name),
        }
    }

    pub fn lookup_username_opt(&self) -> ::errors::Result<Option<String>> {
        api::lookup_username(*self)
    }

    pub fn lookup_username(&self) -> ::errors::Result<String> {
        use errors::*;
        self.lookup_username_opt().and_then(|x| x.chain_err(|| "Could not find Roblox user."))
    }
}