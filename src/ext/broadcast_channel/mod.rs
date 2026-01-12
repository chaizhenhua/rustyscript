use deno_core::{extension, Extension};
use deno_web::InMemoryBroadcastChannel;

use super::ExtensionTrait;

mod wrapper;
pub use wrapper::{BroadcastChannel, BroadcastChannelWrapper};

extension!(
    init_broadcast_channel,
    deps = [rustyscript],
    esm_entry_point = "ext:init_broadcast_channel/init_broadcast_channel.js",
    esm = [ dir "src/ext/broadcast_channel", "init_broadcast_channel.js" ],
);

extension!(
    deno_broadcast_channel,
    deps = [deno_web],
    esm = [ dir "src/ext/broadcast_channel", "01_broadcast_channel.js" ],
);

impl ExtensionTrait<()> for init_broadcast_channel {
    fn init((): ()) -> Extension {
        init_broadcast_channel::init()
    }
}

impl ExtensionTrait<()> for deno_broadcast_channel {
    fn init((): ()) -> Extension {
        deno_broadcast_channel::init()
    }
}

// Note: broadcast_channel functionality is now integrated into deno_web
// No separate initialization is needed as it's handled by deno_web extension
pub fn extensions(_channel: InMemoryBroadcastChannel, is_snapshot: bool) -> Vec<Extension> {
    vec![
        deno_broadcast_channel::build((), is_snapshot),
        init_broadcast_channel::build((), is_snapshot),
    ]
}
