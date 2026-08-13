//! The concrete enactors plugged into the broker's `EnactorRegistry`. The
//! platform-neutral `time.extend` enactor lives in the shared spine
//! (`charter_spine::enactors`) and is re-exported here under its historical
//! path; the Linux-only enactors (Flatpak install, exec-allow) stay local.

pub mod exec_allow;
pub mod install_flatpak;
pub use charter_spine::enactors::{app_open, time_extend};

pub use app_open::AppOpenEnactor;
pub use exec_allow::{
    build_exec_allow_request, plan_exec_allow, sanitize_name_from_path, ExecAllowEnactor,
    ExecRequest, ExecRequestError,
};
pub use install_flatpak::{
    build_install_request, tightened_override_args, InstallFlatpakEnactor, InstallRequest,
    RequestBuildError,
};
pub use time_extend::TimeExtendEnactor;
pub mod learning_apps;
