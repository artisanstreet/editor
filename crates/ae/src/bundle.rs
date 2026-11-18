use artisan_paths::App;
use artisan_payload::Payload;

pub static FORGE: Payload<'static> = artisan_payload::payload!("forge.gz");
static FORGE_NOTICES: Payload<'static> = artisan_payload::payload!("forge-notices.gz");
static AE_NOTICES: Payload<'static> = artisan_payload::payload!("ae-notices.gz");

pub const fn notices(app: App) -> Option<Payload<'static>> {
    match app {
        App::Forge => Some(FORGE_NOTICES),
        App::Ae => Some(AE_NOTICES),
        App::Editor => None,
    }
}
