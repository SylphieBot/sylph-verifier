mod api;
mod lz4;
mod place;
mod rules;

pub use self::place::{create_place_file, LuaConfigEntry, LuaConfigValue};
pub use self::rules::{VerificationRule, VerificationSet, RuleResult};

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct RobloxUserID(pub u64);
impl RobloxUserID {
    pub fn for_username(name: &str) -> ::errors::Result<RobloxUserID> {
        match api::for_username(name)? {
            Some(id) => Ok(id),
            None => cmd_error!("No Roblox user named that was found."),
        }
    }

    pub fn lookup_username_opt(&self) -> ::errors::Result<Option<String>> {
        api::lookup_username(*self)
    }

    pub fn lookup_username(&self) -> ::errors::Result<String> {
        Ok(self.lookup_username_opt()??)
    }
}