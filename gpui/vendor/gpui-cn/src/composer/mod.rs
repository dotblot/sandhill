//! The composer family: the attachment strip, the permission and model
//! pickers, the status tab, and the composer that holds them.

mod attachment;
mod effort;
mod icons;
mod model_picker;
mod permission;
mod prompt;
mod status;
mod status_select;

pub use attachment::{
    Attachment, AttachmentKind, AttachmentStatus, AttachmentStrip, AttachmentTile,
};
pub use effort::EffortMenu;
pub use icons::ComposerAssets;
pub(crate) use model_picker::init as init_model_picker;
pub use model_picker::{
    EFFORT_LABELS, ModelEntry, ModelPicker, ModelPickerEvent, ModelPickerState, ModelProvider,
    ModelView,
};
pub use permission::{
    PermissionEvent, PermissionMenu, PermissionMode, PermissionState, PermissionTone,
};
pub use prompt::{Composer, ComposerEvent, ComposerState};
pub use status::{ComposerStatusTab, ContextMeter};
pub use status_select::{
    SEARCH_MIN_OPTIONS, StatusOption, StatusSelect, StatusSelectEvent, StatusSelectState,
};
