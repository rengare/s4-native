//! Every lifted module, in the order dispatchers are tried.

/// The dispatchers of all lifted modules.
pub fn dispatchers() -> Vec<s4_x86::Dispatch> {
    vec![
        lifted_s4main::dispatch,
        lifted_gfxengine::dispatch,
        lifted_guiengine2::dispatch,
        lifted_soundengine::dispatch,
        lifted_eventengine::dispatch,
        lifted_mapaccess::dispatch,
        lifted_savegameaccess::dispatch,
        lifted_videoengine::dispatch,
        lifted_s4bbgcclient::dispatch,
        lifted_bbsupport::dispatch,
        lifted_economyai::dispatch,
        lifted_mapgenerator::dispatch,
        lifted_rtcomp::dispatch,
        lifted_utility::dispatch,
        lifted_msvcrt::dispatch,
        lifted_msvcp60::dispatch,
        lifted_msvcirt::dispatch,
        lifted_binkw32::dispatch,
    ]
}
