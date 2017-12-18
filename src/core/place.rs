use core::VerifierCore;
use core::config::*;
use errors::*;
use parking_lot::Mutex;
use roblox::*;
use sha2::*;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

struct PlaceManagerState {
    place_target: PathBuf, current_hash: [u8; 32],
}
pub struct PlaceManager(Mutex<PlaceManagerState>);
impl PlaceManager {
    pub fn new(place_target: PathBuf) -> Result<PlaceManager> {
        let current_hash = if place_target.exists() {
            let mut handle = File::open(&place_target)?;
            let mut vec = Vec::new();
            handle.read_to_end(&mut vec)?;

            let mut arr = [0u8; 32];
            arr.copy_from_slice(Sha256::digest(&vec).as_slice());
            arr
        } else {
            [0u8; 32]
        };
        Ok(PlaceManager(Mutex::new(PlaceManagerState { place_target, current_hash })))
    }

    fn place_config(&self, core: &VerifierCore) -> Result<Vec<LuaConfigEntry>> {
        let mut config = Vec::new();
        config.push(LuaConfigEntry::new("title", false,
                                        core.config().get(None, ConfigKeys::PlaceUITitle)?));
        config.push(LuaConfigEntry::new("intro_text", false,
                                        core.config().get(None, ConfigKeys::PlaceUIInstructions)?));
        config.push(LuaConfigEntry::new("bot_prefix", false,
                                        core.config().get(None, ConfigKeys::CommandPrefix)?));
        config.push(LuaConfigEntry::new("background_image", false,
                                        core.config().get(None, ConfigKeys::PlaceUIBackground)?));
        core.verifier().add_config(&mut config);
        Ok(config)
    }
    fn check_write_place(&self, data: &[u8]) -> Result<()> {
        let state = self.0.lock();

        let hash = Sha256::digest(data);
        if hash.as_slice() != state.current_hash {
            info!("An updated place file has been written to '{}'.", state.place_target.display());
            // TODO: Online documentation!
            info!("Please follow the instructions at [url] to update the place. If you do not, \
                   the verifier bot may not work.");

            let mut handle = File::create(&state.place_target)?;
            handle.write_all(data)?;
        }
        Ok(())
    }
    pub fn update_place(&self, core: &VerifierCore) -> Result<()> {
        let place_data = create_place_file(None, &self.place_config(core)?)?;
        self.check_write_place(&place_data)?;
        Ok(())
    }
}