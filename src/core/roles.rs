use database::*;

pub struct RoleManager {
    database: Database,
}
impl RoleManager {
    pub fn new(database: Database) -> RoleManager {
        RoleManager { database }
    }


}