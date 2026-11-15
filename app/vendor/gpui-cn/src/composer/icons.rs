use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString, assets::Assets};

gpui_kit::assets::icon_assets!(
    ComposerExtras,
    [
        ClipboardList,
        FileCode,
        FileSpreadsheet,
        Gauge,
        GitBranch,
        Hand,
        Image,
        Laptop,
        Lock,
        LockOpen,
        Mic,
        Presentation,
        ShieldAlert,
        ShieldCheck,
        Video,
    ]
);

/// An asset source for an application that uses the composer family: the
/// default `gpui_kit::assets::Assets` plus the Lucide icons the composer's
/// defaults draw, which the default set does not hold (the microphone, the
/// shield, the file kinds, and the git branch).
///
/// An application that does not register it still works: it passes its own
/// icons to the components that take one, and a default icon that is not
/// registered renders nothing.
///
/// ```no_run
/// let app = gpui_kit::application().with_assets(gpui_cn::ComposerAssets);
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct ComposerAssets;

impl AssetSource for ComposerAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ComposerExtras.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ComposerExtras.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
