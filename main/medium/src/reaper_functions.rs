use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr::{null_mut, NonNull};

use reaper_low::raw;

use crate::ProjectContext::CurrentProject;
use crate::{
    require_non_null_panic, ActionValueChange, AddFxBehavior, AutomationMode, Bpm, ChunkCacheHint,
    CommandId, Db, EnvChunkName, FxAddByNameBehavior, FxPresetRef, FxShowInstruction, GangBehavior,
    GlobalAutomationModeOverride, Hwnd, InputMonitoringMode, KbdSectionInfo, MasterTrackBehavior,
    MediaTrack, MessageBoxResult, MessageBoxType, MidiInput, MidiInputDeviceId, MidiOutputDeviceId,
    NotificationBehavior, PlaybackSpeedFactor, ProjectContext, ProjectRef, ReaProject,
    ReaperFunctionError, ReaperFunctionResult, ReaperNormalizedFxParamValue, ReaperPanValue,
    ReaperPointer, ReaperStringArg, ReaperVersion, ReaperVolumeValue, RecordArmMode,
    RecordingInput, SectionContext, SectionId, SendTarget, StuffMidiMessageTarget,
    TrackAttributeKey, TrackDefaultsBehavior, TrackEnvelope, TrackFxChainType, TrackFxLocation,
    TrackRef, TrackSendAttributeKey, TrackSendCategory, TrackSendDirection, TransferBehavior,
    UndoBehavior, UndoScope, ValueChange, VolumeSliderValue, WindowContext,
};

use helgoboss_midi::ShortMessage;
use reaper_low;
use reaper_low::raw::GUID;

use std::fmt::Debug;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::path::PathBuf;

/// Represents a privilege to execute functions which are only safe to execute from the main thread.
pub trait MainThreadOnly: private::Sealed {}

/// A usage scope which unlocks all functions that are safe to execute from the main thread.
#[derive(Debug, Default)]
pub struct MainThreadScope(pub(crate) ());

impl MainThreadOnly for MainThreadScope {}

/// Represents a privilege to execute functions which are only safe to execute from the real-time
/// audio thread.
pub trait AudioThreadOnly: private::Sealed {}

/// A usage scope which unlocks all functions that are safe to execute from the real-time audio
/// thread.
#[derive(Debug)]
pub struct RealTimeAudioThreadScope(pub(crate) ());

impl AudioThreadOnly for RealTimeAudioThreadScope {}

/// This is the main access point for most REAPER functions.
///
/// # Basics
///
/// You can obtain an instance of this struct by calling [`Reaper::functions()`]. This unlocks all
/// functions which are safe to execute in the main thread. If you want access to the functions
/// which are safe to execute in the real-time audio thread, call
/// [`Reaper::create_real_time_functions()`] instead. REAPER functions which are related to
/// registering/unregistering things are located in [`Reaper`].
///
/// Please note that this struct contains nothing but function pointers, so you are free to clone
/// it, e.g. in order to make all functions accessible somewhere else. This is sometimes easier than
/// passing references around. Don't do it too often though. It's just a bitwise copy of all
/// function pointers, but there are around 800 of them, so each copy will occupy about 7 kB of
/// memory on a 64-bit system.
///
/// # Panics
///
/// Don't assume that all REAPER functions exposed here are always available. It's possible that the
/// user runs your plug-in in an older version of REAPER where a function is missing. See the
/// documentation of [low-level `Reaper`] for ways how to deal with this.
///
/// # Work in progress
///
/// Many functions which are available in the low-level API have not been lifted to the medium-level
/// API yet. Unlike the low-level API, the medium-level one is hand-written and probably a perpetual
/// work in progress. If you can't find the function that you need, you can always resort to the
/// low-level API by navigating to [`low()`]. Of course you are welcome to contribute to bring the
/// medium-level API on par with the low-level one.
///
/// # Design
///
/// ## What's the `<MainThreadScope>` in `ReaperFunctions<MainThreadScope>` about?
///
/// In REAPER and probably many other DAWs there are at least two important threads:
///
/// 1. The main thread (responsible for things like UI, driven by the UI main loop).
/// 2. The real-time audio thread (responsible for processing audio and MIDI buffers, driven by the
///    audio hardware)
///
/// Most functions offered by REAPER are only safe to be executed in the main thread. If you execute
/// them in another thread, REAPER will crash. Or worse: It will seemingly work on your machine
/// and crash on someone else's. There are also a few functions which are only safe to be executed
/// in the audio thread. And there are also very few functions which are safe to be executed from
/// *any* thread (thread-safe).
///
/// There's currently no way to make sure at compile time that a function is called in the correct
/// thread. Of course that would be the best. In an attempt to still let the compiler help you a
/// bit, the traits [`MainThreadOnly`] and [`RealTimeAudioThreadOnly`] have been introduced. They
/// are marker traits which are used as type bound on each method which is not thread-safe. So
/// depending on the context we can expose an instance of [`ReaperFunctions`] which has only
/// functions unlocked which are safe to be executed from e.g. the real-time audio thread. The
/// compiler will complain if you attempt to call a real-time-audio-thread-only method on
/// `ReaperFunctions<MainThreadScope>` and vice versa.
///
/// Of course that technique can't prevent anyone from acquiring a main-thread only instance and
/// use it in the audio hook. But still, it adds some extra safety.
///
/// The alternative to tagging functions via marker traits would have been to implement e.g.
/// audio-thread-only functions in a trait `CallableFromRealTimeAudioThread` as default functions
/// and create a struct that inherits those default functions. Disadvantage: Consumer always would
/// have to bring the trait into scope to see the functions. That's confusing. It also would provide
/// less amount of safety.
///
/// ## Why no fail-fast at runtime when getting threading wrong?
///
/// Another thing which could help would be to panic when a main-thread-only function is called in
/// the real-time audio thread or vice versa. This would prevent "it works on my machine" scenarios.
/// However, this is currently not being done because of possible performance implications.
///
/// [`Reaper`]: struct.Reaper.html
/// [`Reaper::functions()`]: struct.Reaper.html#method.functions
/// [`Reaper::create_real_time_functions()`]: struct.Reaper.html#method.create_real_time_functions
/// [`low()`]: #method.low
/// [low-level `Reaper`]: /reaper_low/struct.Reaper.html
/// [`MainThreadOnly`]: trait.MainThreadOnly.html
/// [`RealTimeAudioThreadOnly`]: trait.RealTimeAudioThreadOnly.html
/// [`ReaperFunctions`]: struct.ReaperFunctions.html
#[derive(Clone, Debug, Default)]
pub struct ReaperFunctions<UsageScope = MainThreadScope> {
    low: reaper_low::Reaper,
    p: PhantomData<UsageScope>,
}

impl<UsageScope> ReaperFunctions<UsageScope> {
    pub(crate) fn new(low: reaper_low::Reaper) -> ReaperFunctions<UsageScope> {
        ReaperFunctions {
            low,
            p: PhantomData,
        }
    }

    /// Gives access to the low-level Reaper instance.
    pub fn low(&self) -> &reaper_low::Reaper {
        &self.low
    }

    /// Returns the requested project and optionally its file name.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the file name you want. If you
    /// are not interested in the file name at all, pass 0.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::ProjectRef::Tab;
    ///
    /// let result = reaper.functions().enum_projects(Tab(4), 256).ok_or("No such tab")?;
    /// let project_dir = result.file_path.ok_or("Project not saved yet")?.parent();
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    // TODO-low Like many functions, this is not marked as unsafe - yet it is still unsafe in one
    //  way: It must be called in the main thread, otherwise there will be undefined behavior. For
    //  now, the strategy is to just document it and have the type system help a bit
    //  (`ReaperFunctions<MainThread>`). However, there *is* a way to make it safe in the sense of
    //  failing fast without running into undefined behavior: Assert at each function call that we
    //  are in the main thread. The main thread ID could be easily obtained at construction time
    //  of medium-level Reaper. So all it needs is acquiring the current thread and compare its ID
    //  with the main thread ID (both presumably cheap). I think that would be fine. Maybe we should
    //  provide a feature to turn it on/off or make it a debug_assert only or provide an additional
    //  unchecked version. In audio-thread functions it might be too much overhead though calling
    //  is_in_real_time_audio() each time, so maybe we should mark them as unsafe.
    pub fn enum_projects(
        &self,
        project_ref: ProjectRef,
        buffer_size: u32,
    ) -> Option<EnumProjectsResult>
    where
        UsageScope: MainThreadOnly,
    {
        let idx = project_ref.to_raw();
        if buffer_size == 0 {
            let ptr = unsafe { self.low.EnumProjects(idx, null_mut(), 0) };
            let project = NonNull::new(ptr)?;
            Some(EnumProjectsResult {
                project,
                file_path: None,
            })
        } else {
            let (owned_c_string, ptr) =
                with_string_buffer(buffer_size, |buffer, max_size| unsafe {
                    self.low.EnumProjects(idx, buffer, max_size)
                });
            let project = NonNull::new(ptr)?;
            if owned_c_string.to_bytes().len() == 0 {
                return Some(EnumProjectsResult {
                    project,
                    file_path: None,
                });
            }
            let owned_string = owned_c_string
                .into_string()
                .expect("project file path contains non-UTF8 characters");
            Some(EnumProjectsResult {
                project,
                file_path: Some(PathBuf::from(owned_string)),
            })
        }
    }

    /// Returns the track at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::ProjectContext::CurrentProject;
    ///
    /// let track = reaper.functions().get_track(CurrentProject, 3).ok_or("No such track")?;
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub fn get_track(&self, project: ProjectContext, track_index: u32) -> Option<MediaTrack>
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.get_track_unchecked(project, track_index) }
    }

    /// Like [`get_track()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`get_track()`]: #method.get_track
    pub unsafe fn get_track_unchecked(
        &self,
        project: ProjectContext,
        track_index: u32,
    ) -> Option<MediaTrack>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.GetTrack(project.to_raw(), track_index as i32);
        NonNull::new(ptr)
    }

    /// Checks if the given pointer is still valid.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::ProjectContext::CurrentProject;
    ///
    /// let track = reaper.functions().get_track(CurrentProject, 0).ok_or("No track")?;
    /// let track_is_valid = reaper.functions().validate_ptr_2(CurrentProject, track);
    /// assert!(track_is_valid);
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Returns `true` if the pointer is a valid object of the correct type in the given project.
    /// The project is ignored if the pointer itself is a project.
    pub fn validate_ptr_2<'a>(
        &self,
        project: ProjectContext,
        pointer: impl Into<ReaperPointer<'a>>,
    ) -> bool {
        let pointer = pointer.into();
        unsafe {
            self.low.ValidatePtr2(
                project.to_raw(),
                pointer.ptr_as_void(),
                pointer.key_into_raw().as_ptr(),
            )
        }
    }

    /// Checks if the given pointer is still valid.
    ///
    /// Returns `true` if the pointer is a valid object of the correct type in the current project.
    pub fn validate_ptr<'a>(&self, pointer: impl Into<ReaperPointer<'a>>) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        let pointer = pointer.into();
        unsafe {
            self.low
                .ValidatePtr(pointer.ptr_as_void(), pointer.key_into_raw().as_ptr())
        }
    }

    /// Redraws the arrange view and ruler.
    pub fn update_timeline(&self)
    where
        UsageScope: MainThreadOnly,
    {
        self.low.UpdateTimeline();
    }

    /// Shows a message to the user in the ReaScript console.
    ///
    /// This is also useful for debugging. Send "\n" for newline and "" to clear the console.
    pub fn show_console_msg<'a>(&self, message: impl Into<ReaperStringArg<'a>>) {
        unsafe { self.low.ShowConsoleMsg(message.into().as_ptr()) }
    }

    /// Gets or sets a track attribute.
    ///
    /// Returns the current value if `new_value` is `null_mut()`.
    ///
    /// It's recommended to use one of the convenience functions instead. They all start with
    /// `get_set_media_track_info_` and are more type-safe.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track or invalid new value.
    pub unsafe fn get_set_media_track_info(
        &self,
        track: MediaTrack,
        attribute_key: TrackAttributeKey,
        new_value: *mut c_void,
    ) -> *mut c_void
    where
        UsageScope: MainThreadOnly,
    {
        self.low
            .GetSetMediaTrackInfo(track.as_ptr(), attribute_key.into_raw().as_ptr(), new_value)
    }

    /// Convenience function which returns the given track's parent track (`P_PARTRACK`).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_par_track(
        &self,
        track: MediaTrack,
    ) -> Option<MediaTrack>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_media_track_info(track, TrackAttributeKey::ParTrack, null_mut())
            as *mut raw::MediaTrack;
        NonNull::new(ptr)
    }

    /// Convenience function which returns the given track's parent project (`P_PROJECT`).
    ///
    /// In REAPER < 5.95 this returns `None`.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_project(
        &self,
        track: MediaTrack,
    ) -> Option<ReaProject>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_media_track_info(track, TrackAttributeKey::Project, null_mut())
            as *mut raw::ReaProject;
        NonNull::new(ptr)
    }

    /// Convenience function which grants temporary access to the given track's name (`P_NAME`).
    ///
    /// Returns `None` if the given track is the master track.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use reaper_medium::ProjectContext::CurrentProject;
    /// use std::ffi::CString;
    /// let reaper = reaper_medium::Reaper::default();
    ///
    /// let track = reaper.functions().get_track(CurrentProject, 0).ok_or("no track")?;
    /// let track_name_c_string = unsafe {
    ///     reaper.functions().get_set_media_track_info_get_name(
    ///         track,
    ///         |name| name.to_owned()
    ///     )
    /// };
    /// let track_name = match &track_name_c_string {
    ///     None => "Master track",
    ///     Some(name) => name.to_str()?
    /// };
    /// reaper.functions().show_console_msg(format!("Track name is {}", track_name));
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_name<R>(
        &self,
        track: MediaTrack,
        use_name: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_media_track_info(track, TrackAttributeKey::Name, null_mut());
        create_passing_c_str(ptr as *const c_char).map(use_name)
    }

    /// Convenience function which returns the given track's input monitoring mode (I_RECMON).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_rec_mon(
        &self,
        track: MediaTrack,
    ) -> InputMonitoringMode
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_media_track_info(track, TrackAttributeKey::RecMon, null_mut());
        let irecmon = deref_as::<i32>(ptr).expect("irecmon pointer is null");
        InputMonitoringMode::try_from_raw(irecmon).expect("unknown input monitoring mode")
    }

    /// Convenience function which returns the given track's recording input (I_RECINPUT).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_rec_input(
        &self,
        track: MediaTrack,
    ) -> Option<RecordingInput>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_media_track_info(track, TrackAttributeKey::RecInput, null_mut());
        let rec_input_index = deref_as::<i32>(ptr).expect("rec_input_index pointer is null");
        if rec_input_index < 0 {
            None
        } else {
            Some(RecordingInput::try_from_raw(rec_input_index).expect("unknown recording input"))
        }
    }

    /// Convenience function which returns the type and location of the given track
    /// (IP_TRACKNUMBER).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_track_number(
        &self,
        track: MediaTrack,
    ) -> Option<TrackRef>
    where
        UsageScope: MainThreadOnly,
    {
        use TrackRef::*;
        match self.get_set_media_track_info(track, TrackAttributeKey::TrackNumber, null_mut())
            as i32
        {
            -1 => Some(MasterTrack),
            0 => None,
            n if n > 0 => Some(NormalTrack(n as u32 - 1)),
            _ => unreachable!(),
        }
    }

    /// Convenience function which returns the given track's GUID (GUID).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_set_media_track_info_get_guid(&self, track: MediaTrack) -> GUID
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_media_track_info(track, TrackAttributeKey::Guid, null_mut());
        deref_as::<GUID>(ptr).expect("GUID pointer is null")
    }

    /// Returns whether we are in the real-time audio thread.
    ///
    /// *Real-time* means somewhere between [`OnAudioBuffer`] calls, not in some worker or
    /// anticipative FX thread.
    ///
    /// [`OnAudioBuffer`]: trait.MediumOnAudioBuffer.html#method.call
    pub fn is_in_real_time_audio(&self) -> bool {
        self.low.IsInRealTimeAudio() != 0
    }

    /// Performs an action belonging to the main section.
    ///
    /// To perform non-native actions (ReaScripts, custom or extension plugin actions) safely, see
    /// [`named_command_lookup()`].
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    ///
    /// [`named_command_lookup()`]: #method.named_command_lookup
    pub fn main_on_command_ex(&self, command: CommandId, flag: i32, project: ProjectContext) {
        self.require_valid_project(project);
        unsafe { self.main_on_command_ex_unchecked(command, flag, project) }
    }

    /// Like [`main_on_command_ex()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`main_on_command_ex()`]: #method.main_on_command_ex
    pub unsafe fn main_on_command_ex_unchecked(
        &self,
        command_id: CommandId,
        flag: i32,
        project: ProjectContext,
    ) {
        self.low
            .Main_OnCommandEx(command_id.to_raw(), flag, project.to_raw());
    }

    /// Mutes or unmutes the given track.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::{NotificationBehavior::NotifyAll, ProjectContext::CurrentProject};
    ///
    /// let track = reaper.functions().get_track(CurrentProject, 0).ok_or("no tracks")?;
    /// unsafe {
    ///     reaper.functions().csurf_set_surface_mute(track, true, NotifyAll);
    /// }
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub unsafe fn csurf_set_surface_mute(
        &self,
        track: MediaTrack,
        mute: bool,
        notification_behavior: NotificationBehavior,
    ) {
        self.low
            .CSurf_SetSurfaceMute(track.as_ptr(), mute, notification_behavior.to_raw());
    }

    /// Soloes or unsoloes the given track.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_set_surface_solo(
        &self,
        track: MediaTrack,
        solo: bool,
        notification_behavior: NotificationBehavior,
    ) {
        self.low
            .CSurf_SetSurfaceSolo(track.as_ptr(), solo, notification_behavior.to_raw());
    }

    /// Generates a random GUID.
    pub fn gen_guid(&self) -> GUID
    where
        UsageScope: MainThreadOnly,
    {
        let mut guid = MaybeUninit::uninit();
        unsafe {
            self.low.genGuid(guid.as_mut_ptr());
        }
        unsafe { guid.assume_init() }
    }

    /// Grants temporary access to the section with the given ID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::SectionId;
    ///
    /// let action_count =
    ///     reaper.functions().section_from_unique_id(SectionId::new(1), |s| s.action_list_cnt());
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    //
    // In order to not need unsafe, we take the closure. For normal medium-level API usage, this is
    // the safe way to go.
    pub fn section_from_unique_id<R>(
        &self,
        section_id: SectionId,
        use_section: impl FnOnce(&KbdSectionInfo) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.SectionFromUniqueID(section_id.to_raw());
        if ptr.is_null() {
            return None;
        }
        NonNull::new(ptr).map(|nnp| use_section(&KbdSectionInfo(nnp)))
    }

    /// Like [`section_from_unique_id()`] but returns the section.
    ///
    /// # Safety
    ///
    /// The lifetime of the returned section is unbounded.
    ///
    /// [`section_from_unique_id()`]: #method.section_from_unique_id
    // The closure-taking function might be too restrictive in some cases, e.g. it wouldn't let us
    // return an iterator (which is of course lazily evaluated). Also, in some cases we might know
    // that a section is always valid, e.g. if it's the main section. A higher-level API could
    // use this for such edge cases. If not the main section, a higher-level API
    // should check if the section still exists (via section index) before each usage.
    pub unsafe fn section_from_unique_id_unchecked(
        &self,
        section_id: SectionId,
    ) -> Option<KbdSectionInfo>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.SectionFromUniqueID(section_id.to_raw());
        NonNull::new(ptr).map(KbdSectionInfo)
    }

    /// Performs an action belonging to the main section.
    ///
    /// Unlike [`main_on_command_ex()`], this function also allows to control actions learned with
    /// MIDI/OSC.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project or window.
    ///
    /// [`main_on_command_ex()`]: #method.main_on_command_ex
    // Kept return value type i32 because I have no idea what the return value is about.
    pub unsafe fn kbd_on_main_action_ex(
        &self,
        command_id: CommandId,
        value_change: ActionValueChange,
        window: WindowContext,
        project: ProjectContext,
    ) -> i32
    where
        UsageScope: MainThreadOnly,
    {
        use ActionValueChange::*;
        let (val, valhw, relmode) = match value_change {
            AbsoluteLowRes(v) => (i32::from(v), -1, 0),
            AbsoluteHighRes(v) => (
                ((u32::from(v) >> 7) & 0x7f) as i32,
                (u32::from(v) & 0x7f) as i32,
                0,
            ),
            Relative1(v) => (i32::from(v), -1, 1),
            Relative2(v) => (i32::from(v), -1, 2),
            Relative3(v) => (i32::from(v), -1, 3),
        };
        self.low.KBD_OnMainActionEx(
            command_id.to_raw(),
            val,
            valhw,
            relmode,
            window.to_raw(),
            project.to_raw(),
        )
    }

    /// Returns the REAPER main window handle.
    pub fn get_main_hwnd(&self) -> Hwnd
    where
        UsageScope: MainThreadOnly,
    {
        require_non_null_panic(self.low.GetMainHwnd())
    }

    /// Looks up the command ID for a named command.
    ///
    /// Named commands can be registered by extensions (e.g. `_SWS_ABOUT`), ReaScripts
    /// (e.g. `_113088d11ae641c193a2b7ede3041ad5`) or custom actions.
    pub fn named_command_lookup<'a>(
        &self,
        command_name: impl Into<ReaperStringArg<'a>>,
    ) -> Option<CommandId>
    where
        UsageScope: MainThreadOnly,
    {
        let raw_id = unsafe { self.low.NamedCommandLookup(command_name.into().as_ptr()) as u32 };
        if raw_id == 0 {
            return None;
        }
        Some(CommandId(raw_id))
    }

    /// Clears the ReaScript console.
    pub fn clear_console(&self) {
        self.low.ClearConsole();
    }

    /// Returns the number of tracks in the given project.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn count_tracks(&self, project: ProjectContext) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.count_tracks_unchecked(project) }
    }

    /// Like [`count_tracks()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`count_tracks()`]: #method.count_tracks
    pub unsafe fn count_tracks_unchecked(&self, project: ProjectContext) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.CountTracks(project.to_raw()) as u32
    }

    /// Creates a new track at the given index.
    pub fn insert_track_at_index(&self, index: u32, defaults_behavior: TrackDefaultsBehavior) {
        self.low.InsertTrackAtIndex(
            index as i32,
            defaults_behavior == TrackDefaultsBehavior::AddDefaultEnvAndFx,
        );
    }

    /// Returns the maximum number of MIDI input devices (usually 63).
    pub fn get_max_midi_inputs(&self) -> u32 {
        self.low.GetMaxMidiInputs() as u32
    }

    /// Returns the maximum number of MIDI output devices (usually 64).
    pub fn get_max_midi_outputs(&self) -> u32 {
        self.low.GetMaxMidiOutputs() as u32
    }

    /// Returns information about the given MIDI input device.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the device name you want.
    /// If you are not interested in the device name at all, pass 0.
    pub fn get_midi_input_name(
        &self,
        device_id: MidiInputDeviceId,
        buffer_size: u32,
    ) -> GetMidiDevNameResult
    where
        UsageScope: MainThreadOnly,
    {
        if buffer_size == 0 {
            let is_present =
                unsafe { self.low.GetMIDIInputName(device_id.to_raw(), null_mut(), 0) };
            GetMidiDevNameResult {
                is_present,
                name: None,
            }
        } else {
            let (name, is_present) = with_string_buffer(buffer_size, |buffer, max_size| unsafe {
                self.low
                    .GetMIDIInputName(device_id.to_raw(), buffer, max_size)
            });
            if name.to_bytes().len() == 0 {
                return GetMidiDevNameResult {
                    is_present,
                    name: None,
                };
            }
            GetMidiDevNameResult {
                is_present,
                name: Some(name),
            }
        }
    }

    /// Returns information about the given MIDI output device.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the device name you want.
    /// If you are not interested in the device name at all, pass 0.
    pub fn get_midi_output_name(
        &self,
        device_id: MidiOutputDeviceId,
        buffer_size: u32,
    ) -> GetMidiDevNameResult
    where
        UsageScope: MainThreadOnly,
    {
        if buffer_size == 0 {
            let is_present = unsafe {
                self.low
                    .GetMIDIOutputName(device_id.to_raw(), null_mut(), 0)
            };
            GetMidiDevNameResult {
                is_present,
                name: None,
            }
        } else {
            let (name, is_present) = with_string_buffer(buffer_size, |buffer, max_size| unsafe {
                self.low
                    .GetMIDIOutputName(device_id.to_raw(), buffer, max_size)
            });
            if name.to_bytes().len() == 0 {
                return GetMidiDevNameResult {
                    is_present,
                    name: None,
                };
            }
            GetMidiDevNameResult {
                is_present,
                name: Some(name),
            }
        }
    }

    // Return type Option or Result can't be easily chosen here because if instantiate is 0, it
    // should be Option, if it's -1 or > 0, it should be Result. So we just keep the i32. That's
    // also one reason why we just publish the convenience functions.
    unsafe fn track_fx_add_by_name<'a>(
        &self,
        track: MediaTrack,
        fx_name: impl Into<ReaperStringArg<'a>>,
        fx_chain_type: TrackFxChainType,
        behavior: FxAddByNameBehavior,
    ) -> i32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.TrackFX_AddByName(
            track.as_ptr(),
            fx_name.into().as_ptr(),
            fx_chain_type == TrackFxChainType::InputFxChain,
            behavior.to_raw(),
        )
    }

    /// Returns the index of the first FX instance in a track or monitoring FX chain.
    ///
    /// The FX name can have a prefix to further specify its type: `VST3:` | `VST2:` | `VST:` |
    /// `AU:` | `JS:` | `DX:`
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_add_by_name_query<'a>(
        &self,
        track: MediaTrack,
        fx_name: impl Into<ReaperStringArg<'a>>,
        fx_chain_type: TrackFxChainType,
    ) -> Option<u32>
    where
        UsageScope: MainThreadOnly,
    {
        match self.track_fx_add_by_name(track, fx_name, fx_chain_type, FxAddByNameBehavior::Query) {
            -1 => None,
            idx if idx >= 0 => Some(idx as u32),
            _ => unreachable!(),
        }
    }

    /// Adds an instance of an FX to a track or monitoring FX chain.
    ///
    /// See [`track_fx_add_by_name_query()`] for possible FX name prefixes.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX couldn't be added (e.g. if no such FX is installed).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    ///
    /// [`track_fx_add_by_name_query()`]: #method.track_fx_add_by_name_query
    pub unsafe fn track_fx_add_by_name_add<'a>(
        &self,
        track: MediaTrack,
        fx_name: impl Into<ReaperStringArg<'a>>,
        fx_chain_type: TrackFxChainType,
        behavior: AddFxBehavior,
    ) -> ReaperFunctionResult<u32>
    where
        UsageScope: MainThreadOnly,
    {
        match self.track_fx_add_by_name(track, fx_name, fx_chain_type, behavior.into()) {
            -1 => Err(ReaperFunctionError::new("FX couldn't be added")),
            idx if idx >= 0 => Ok(idx as u32),
            _ => unreachable!(),
        }
    }

    /// Returns whether the given track FX is enabled.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_enabled(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
    ) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.low
            .TrackFX_GetEnabled(track.as_ptr(), fx_location.to_raw())
    }

    /// Returns the name of the given FX.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the FX name you want.
    ///
    /// # Panics
    ///
    /// Panics if the given buffer size is 0.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_fx_name(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        buffer_size: u32,
    ) -> ReaperFunctionResult<CString>
    where
        UsageScope: MainThreadOnly,
    {
        assert!(buffer_size > 0);
        let (name, successful) = with_string_buffer(buffer_size, |buffer, max_size| {
            self.low
                .TrackFX_GetFXName(track.as_ptr(), fx_location.to_raw(), buffer, max_size)
        });
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't get FX name (probably FX doesn't exist)",
            ));
        }
        Ok(name)
    }

    /// Returns the index of the first track FX that is a virtual instrument.
    ///
    /// Doesn't look in the input FX chain.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_instrument(&self, track: MediaTrack) -> Option<u32>
    where
        UsageScope: MainThreadOnly,
    {
        let index = self.low.TrackFX_GetInstrument(track.as_ptr());
        if index == -1 {
            return None;
        }
        Some(index as u32)
    }

    /// Enables or disables a track FX.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_set_enabled(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        is_enabled: bool,
    ) {
        self.low
            .TrackFX_SetEnabled(track.as_ptr(), fx_location.to_raw(), is_enabled);
    }

    /// Returns the number of parameters of given track FX.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_num_params(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
    ) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.low
            .TrackFX_GetNumParams(track.as_ptr(), fx_location.to_raw()) as u32
    }

    /// Returns the current project if it's just being loaded or saved.
    ///
    /// This is usually only used from `project_config_extension_t`.
    // TODO-low `project_config_extension_t` is not yet ported
    pub fn get_current_project_in_load_save(&self) -> Option<ReaProject>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.GetCurrentProjectInLoadSave();
        NonNull::new(ptr)
    }

    /// Returns the name of the given track FX parameter.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the parameter name you want.
    ///
    /// # Panics
    ///
    /// Panics if the given buffer size is 0.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX or parameter doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_param_name(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
        buffer_size: u32,
    ) -> ReaperFunctionResult<CString>
    where
        UsageScope: MainThreadOnly,
    {
        assert!(buffer_size > 0);
        let (name, successful) = with_string_buffer(buffer_size, |buffer, max_size| {
            self.low.TrackFX_GetParamName(
                track.as_ptr(),
                fx_location.to_raw(),
                param_index as i32,
                buffer,
                max_size,
            )
        });
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't get FX parameter name (probably FX or parameter doesn't exist)",
            ));
        }
        Ok(name)
    }

    /// Returns the current value of the given track FX parameter formatted as string.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the parameter value string you
    /// want.
    ///
    /// # Panics
    ///
    /// Panics if the given buffer size is 0.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX or parameter doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_formatted_param_value(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
        buffer_size: u32,
    ) -> ReaperFunctionResult<CString>
    where
        UsageScope: MainThreadOnly,
    {
        assert!(buffer_size > 0);
        let (name, successful) = with_string_buffer(buffer_size, |buffer, max_size| {
            self.low.TrackFX_GetFormattedParamValue(
                track.as_ptr(),
                fx_location.to_raw(),
                param_index as i32,
                buffer,
                max_size,
            )
        });
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't format current FX parameter value (probably FX or parameter doesn't exist)",
            ));
        }
        Ok(name)
    }

    /// Returns the given value formatted as string according to the given track FX parameter.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the parameter value string you
    /// want.
    ///
    /// This only works with FX that supports Cockos VST extensions.
    ///
    /// # Panics
    ///
    /// Panics if the given buffer size is 0.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX or parameter doesn't exist. Also errors if the FX doesn't support
    /// formatting arbitrary parameter values *and* the given value is not equal to the current
    /// one. If the given value is equal to the current one, it's just like calling
    /// [`track_fx_get_formatted_param_value`].
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    ///
    /// [`track_fx_get_formatted_param_value`]: #method.track_fx_get_formatted_param_value
    pub unsafe fn track_fx_format_param_value_normalized(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
        param_value: ReaperNormalizedFxParamValue,
        buffer_size: u32,
    ) -> ReaperFunctionResult<CString>
    where
        UsageScope: MainThreadOnly,
    {
        assert!(buffer_size > 0);
        let (name, successful) = with_string_buffer(buffer_size, |buffer, max_size| {
            self.low.TrackFX_FormatParamValueNormalized(
                track.as_ptr(),
                fx_location.to_raw(),
                param_index as i32,
                param_value.get(),
                buffer,
                max_size,
            )
        });
        if !successful {
            "FX or FX parameter not found or Cockos extensions not supported";
            return Err(ReaperFunctionError::new(
                "couldn't format FX parameter value (FX maybe doesn't support Cockos extensions or FX or parameter doesn't exist)",
            ));
        }
        Ok(name)
    }

    /// Sets the value of the given track FX parameter.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX or parameter doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_set_param_normalized(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
        param_value: ReaperNormalizedFxParamValue,
    ) -> ReaperFunctionResult<()>
    where
        UsageScope: MainThreadOnly,
    {
        let successful = self.low.TrackFX_SetParamNormalized(
            track.as_ptr(),
            fx_location.to_raw(),
            param_index as i32,
            param_value.get(),
        );
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't set FX parameter value (probably FX or parameter doesn't exist)",
            ));
        }
        Ok(())
    }

    /// Returns information about the (last) focused FX window.
    ///
    /// Returns `Some` if an FX window has focus or was the last focused one and is still open.
    /// Returns `None` if no FX window has focus.
    pub fn get_focused_fx(&self) -> Option<GetFocusedFxResult>
    where
        UsageScope: MainThreadOnly,
    {
        let mut tracknumber = MaybeUninit::uninit();
        let mut itemnumber = MaybeUninit::uninit();
        let mut fxnumber = MaybeUninit::uninit();
        let result = unsafe {
            self.low.GetFocusedFX(
                tracknumber.as_mut_ptr(),
                itemnumber.as_mut_ptr(),
                fxnumber.as_mut_ptr(),
            )
        };
        let tracknumber = unsafe { tracknumber.assume_init() as u32 };
        let fxnumber = unsafe { fxnumber.assume_init() };
        use GetFocusedFxResult::*;
        match result {
            0 => None,
            1 => Some(TrackFx {
                track_ref: convert_tracknumber_to_track_ref(tracknumber),
                fx_location: TrackFxLocation::try_from_raw(fxnumber)
                    .expect("unknown track FX location"),
            }),
            2 => {
                // TODO-low Add test
                let fxnumber = fxnumber as u32;
                Some(TakeFx {
                    // Master track can't contain items
                    track_index: tracknumber - 1,
                    // Although the parameter is called itemnumber, it's zero-based (mentioned in
                    // official doc and checked)
                    item_index: unsafe { itemnumber.assume_init() as u32 },
                    take_index: (fxnumber >> 16) & 0xFFFF,
                    fx_index: fxnumber & 0xFFFF,
                })
            }
            _ => panic!("Unknown GetFocusedFX result value"),
        }
    }

    /// Returns information about the last touched FX parameter.
    ///
    /// Returns `Some` if an FX parameter has been touched already and that FX is still existing.
    /// Returns `None` otherwise.
    pub fn get_last_touched_fx(&self) -> Option<GetLastTouchedFxResult>
    where
        UsageScope: MainThreadOnly,
    {
        let mut tracknumber = MaybeUninit::uninit();
        let mut fxnumber = MaybeUninit::uninit();
        let mut paramnumber = MaybeUninit::uninit();
        let is_valid = unsafe {
            self.low.GetLastTouchedFX(
                tracknumber.as_mut_ptr(),
                fxnumber.as_mut_ptr(),
                paramnumber.as_mut_ptr(),
            )
        };
        if !is_valid {
            return None;
        }
        let tracknumber = unsafe { tracknumber.assume_init() as u32 };
        let tracknumber_high_word = (tracknumber >> 16) & 0xFFFF;
        let fxnumber = unsafe { fxnumber.assume_init() };
        let paramnumber = unsafe { paramnumber.assume_init() as u32 };
        use GetLastTouchedFxResult::*;
        if tracknumber_high_word == 0 {
            Some(TrackFx {
                track_ref: convert_tracknumber_to_track_ref(tracknumber),
                fx_location: TrackFxLocation::try_from_raw(fxnumber)
                    .expect("unknown track FX location"),
                // Although the parameter is called paramnumber, it's zero-based (checked)
                param_index: paramnumber,
            })
        } else {
            // TODO-low Add test
            let fxnumber = fxnumber as u32;
            Some(TakeFx {
                // Master track can't contain items
                track_index: (tracknumber & 0xFFFF) - 1,
                item_index: tracknumber_high_word - 1,
                take_index: (fxnumber >> 16) & 0xFFFF,
                fx_index: fxnumber & 0xFFFF,
                // Although the parameter is called paramnumber, it's zero-based (checked)
                param_index: paramnumber,
            })
        }
    }

    /// Copies, moves or reorders FX.
    ///
    /// Reorders if source and destination track are the same.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_copy_to_track(
        &self,
        source: (MediaTrack, TrackFxLocation),
        destination: (MediaTrack, TrackFxLocation),
        transfer_behavior: TransferBehavior,
    ) {
        self.low.TrackFX_CopyToTrack(
            source.0.as_ptr(),
            source.1.to_raw(),
            destination.0.as_ptr(),
            destination.1.to_raw(),
            transfer_behavior == TransferBehavior::Move,
        );
    }

    /// Removes the given FX from the track FX chain.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_delete(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
    ) -> ReaperFunctionResult<()>
    where
        UsageScope: MainThreadOnly,
    {
        let succesful = self
            .low
            .TrackFX_Delete(track.as_ptr(), fx_location.to_raw());
        if !succesful {
            return Err(ReaperFunctionError::new(
                "couldn't delete FX (probably FX doesn't exist)",
            ));
        }
        Ok(())
    }

    /// Returns information about the given FX parameter's step sizes.
    ///
    /// Returns `None` if the FX parameter doesn't report step sizes or if the FX or parameter
    /// doesn't exist (there's no way to distinguish with just this function).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    //
    // Option makes more sense than Result here because this function is at the same time the
    // correct function to be used to determine *if* a parameter reports step sizes. So
    // "parameter doesn't report step sizes" is a valid result.
    pub unsafe fn track_fx_get_parameter_step_sizes(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
    ) -> Option<GetParameterStepSizesResult>
    where
        UsageScope: MainThreadOnly,
    {
        let mut step = MaybeUninit::uninit();
        let mut small_step = MaybeUninit::uninit();
        let mut large_step = MaybeUninit::uninit();
        let mut is_toggle = MaybeUninit::uninit();
        let successful = self.low.TrackFX_GetParameterStepSizes(
            track.as_ptr(),
            fx_location.to_raw(),
            param_index as i32,
            step.as_mut_ptr(),
            small_step.as_mut_ptr(),
            large_step.as_mut_ptr(),
            is_toggle.as_mut_ptr(),
        );
        if !successful {
            return None;
        }
        let is_toggle = is_toggle.assume_init();
        if is_toggle {
            Some(GetParameterStepSizesResult::Toggle)
        } else {
            Some(GetParameterStepSizesResult::Normal {
                normal_step: step.assume_init(),
                small_step: make_some_if_greater_than_zero(small_step.assume_init()),
                large_step: make_some_if_greater_than_zero(large_step.assume_init()),
            })
        }
    }

    /// Returns the current value and min/mid/max values of the given track FX.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_param_ex(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
    ) -> GetParamExResult
    where
        UsageScope: MainThreadOnly,
    {
        let mut min_val = MaybeUninit::uninit();
        let mut max_val = MaybeUninit::uninit();
        let mut mid_val = MaybeUninit::uninit();
        let value = self.low.TrackFX_GetParamEx(
            track.as_ptr(),
            fx_location.to_raw(),
            param_index as i32,
            min_val.as_mut_ptr(),
            max_val.as_mut_ptr(),
            mid_val.as_mut_ptr(),
        );
        GetParamExResult {
            current_value: value,
            min_value: min_val.assume_init(),
            mid_value: mid_val.assume_init(),
            max_value: max_val.assume_init(),
        }
        .into()
    }

    /// Starts a new undo block.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::{ProjectContext::CurrentProject, UndoScope::Scoped, ProjectPart::*};
    ///
    /// reaper.functions().undo_begin_block_2(CurrentProject);
    /// // ... modify something ...
    /// reaper.functions().undo_end_block_2(CurrentProject, "Modify something", Scoped(Items | Fx));
    /// ```
    pub fn undo_begin_block_2(&self, project: ProjectContext) {
        self.require_valid_project(project);
        unsafe { self.undo_begin_block_2_unchecked(project) };
    }

    /// Like [`undo_begin_block_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`undo_begin_block_2()`]: #method.undo_begin_block_2
    pub unsafe fn undo_begin_block_2_unchecked(&self, project: ProjectContext) {
        self.low.Undo_BeginBlock2(project.to_raw());
    }

    /// Ends the current undo block.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn undo_end_block_2<'a>(
        &self,
        project: ProjectContext,
        description: impl Into<ReaperStringArg<'a>>,
        scope: UndoScope,
    ) {
        self.require_valid_project(project);
        unsafe {
            self.undo_end_block_2_unchecked(project, description, scope);
        }
    }

    /// Like [`undo_end_block_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`undo_end_block_2()`]: #method.undo_end_block_2
    pub unsafe fn undo_end_block_2_unchecked<'a>(
        &self,
        project: ProjectContext,
        description: impl Into<ReaperStringArg<'a>>,
        scope: UndoScope,
    ) {
        self.low.Undo_EndBlock2(
            project.to_raw(),
            description.into().as_ptr(),
            scope.to_raw(),
        );
    }

    /// Grants temporary access to the the description of the last undoable operation, if any.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn undo_can_undo_2<R>(
        &self,
        project: ProjectContext,
        use_description: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.undo_can_undo_2_unchecked(project, use_description) }
    }

    /// Like [`undo_can_undo_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`undo_can_undo_2()`]: #method.undo_can_undo_2
    pub unsafe fn undo_can_undo_2_unchecked<R>(
        &self,
        project: ProjectContext,
        use_description: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.Undo_CanUndo2(project.to_raw());
        create_passing_c_str(ptr).map(use_description)
    }

    /// Grants temporary access to the description of the next redoable operation, if any.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn undo_can_redo_2<R>(
        &self,
        project: ProjectContext,
        use_description: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.undo_can_redo_2_unchecked(project, use_description) }
    }

    /// Like [`undo_can_redo_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`undo_can_redo_2()`]: #method.undo_can_redo_2
    pub unsafe fn undo_can_redo_2_unchecked<R>(
        &self,
        project: ProjectContext,
        use_description: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.Undo_CanRedo2(project.to_raw());
        create_passing_c_str(ptr).map(use_description)
    }

    /// Makes the last undoable operation undone.
    ///
    /// Returns `false` if there was nothing to be undone.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn undo_do_undo_2(&self, project: ProjectContext) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.undo_do_undo_2_unchecked(project) }
    }

    /// Like [`undo_do_undo_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`undo_do_undo_2()`]: #method.undo_do_undo_2
    pub unsafe fn undo_do_undo_2_unchecked(&self, project: ProjectContext) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.low.Undo_DoUndo2(project.to_raw()) != 0
    }

    /// Executes the next redoable action.
    ///
    /// Returns `false` if there was nothing to be redone.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn undo_do_redo_2(&self, project: ProjectContext) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.undo_do_redo_2_unchecked(project) }
    }

    /// Like [`undo_do_redo_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`undo_do_redo_2()`]: #method.undo_do_redo_2
    pub unsafe fn undo_do_redo_2_unchecked(&self, project: ProjectContext) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.low.Undo_DoRedo2(project.to_raw()) != 0
    }

    /// Marks the given project as dirty.
    ///
    /// *Dirty* means the project needs to be saved. Only makes a difference if "Maximum undo
    /// memory" is not 0 in REAPER's preferences (0 disables undo/prompt to save).
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn mark_project_dirty(&self, project: ProjectContext) {
        self.require_valid_project(project);
        unsafe {
            self.mark_project_dirty_unchecked(project);
        }
    }

    /// Like [`mark_project_dirty()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`mark_project_dirty()`]: #method.mark_project_dirty
    pub unsafe fn mark_project_dirty_unchecked(&self, project: ProjectContext) {
        self.low.MarkProjectDirty(project.to_raw());
    }

    /// Returns whether the given project is dirty.
    ///
    /// Always returns `false` if "Maximum undo memory" is 0 in REAPER's preferences.
    ///
    /// Also see [`mark_project_dirty()`]
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    ///
    /// [`mark_project_dirty()`]: #method.mark_project_dirty
    pub fn is_project_dirty(&self, project: ProjectContext) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.is_project_dirty_unchecked(project) }
    }

    /// Like [`is_project_dirty()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`is_project_dirty()`]: #method.is_project_dirty
    pub unsafe fn is_project_dirty_unchecked(&self, project: ProjectContext) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.low.IsProjectDirty(project.to_raw()) != 0
    }

    /// Notifies all control surfaces that something in the track list has changed.
    ///
    /// Behavior not confirmed.
    pub fn track_list_update_all_external_surfaces(&self) {
        self.low.TrackList_UpdateAllExternalSurfaces();
    }

    /// Returns the version of the REAPER application in which this plug-in is currently running.
    pub fn get_app_version(&self) -> ReaperVersion<'static>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.GetAppVersion();
        let version_str = unsafe { CStr::from_ptr(ptr) };
        ReaperVersion::new(version_str)
    }

    /// Returns the track automation mode, regardless of the global override.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_track_automation_mode(&self, track: MediaTrack) -> AutomationMode
    where
        UsageScope: MainThreadOnly,
    {
        let result = self.low.GetTrackAutomationMode(track.as_ptr());
        AutomationMode::try_from_raw(result).expect("unknown automation mode")
    }

    /// Returns the global track automation override, if any.
    pub fn get_global_automation_override(&self) -> Option<GlobalAutomationModeOverride>
    where
        UsageScope: MainThreadOnly,
    {
        use GlobalAutomationModeOverride::*;
        match self.low.GetGlobalAutomationOverride() {
            -1 => None,
            6 => Some(Bypass),
            x => Some(Mode(
                AutomationMode::try_from_raw(x).expect("unknown automation mode"),
            )),
        }
    }

    /// Returns the track envelope for the given track and configuration chunk name.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    // TODO-low Test
    pub unsafe fn get_track_envelope_by_chunk_name(
        &self,
        track: MediaTrack,
        chunk_name: EnvChunkName,
    ) -> Option<TrackEnvelope>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self
            .low
            .GetTrackEnvelopeByChunkName(track.as_ptr(), chunk_name.into_raw().as_ptr());
        NonNull::new(ptr)
    }

    /// Returns the track envelope for the given track and envelope display name.
    ///
    /// For getting common envelopes (like volume or pan) using
    /// [`get_track_envelope_by_chunk_name()`] is better because it provides more type safety.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    ///
    /// [`get_track_envelope_by_chunk_name()`]: #method.get_track_envelope_by_chunk_name
    pub unsafe fn get_track_envelope_by_name<'a>(
        &self,
        track: MediaTrack,
        env_name: impl Into<ReaperStringArg<'a>>,
    ) -> Option<TrackEnvelope>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self
            .low
            .GetTrackEnvelopeByName(track.as_ptr(), env_name.into().as_ptr());
        NonNull::new(ptr)
    }

    /// Gets a track attribute as numerical value.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_media_track_info_value(
        &self,
        track: MediaTrack,
        attribute_key: TrackAttributeKey,
    ) -> f64
    where
        UsageScope: MainThreadOnly,
    {
        self.low
            .GetMediaTrackInfo_Value(track.as_ptr(), attribute_key.into_raw().as_ptr())
    }

    /// Gets the number of FX instances on the given track's normal FX chain.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_count(&self, track: MediaTrack) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.TrackFX_GetCount(track.as_ptr()) as u32
    }

    /// Gets the number of FX instances on the given track's input FX chain.
    ///
    /// On the master track, this refers to the monitoring FX chain.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_rec_count(&self, track: MediaTrack) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.TrackFX_GetRecCount(track.as_ptr()) as u32
    }

    /// Returns the GUID of the given track FX.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_fx_guid(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
    ) -> ReaperFunctionResult<GUID>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self
            .low
            .TrackFX_GetFXGUID(track.as_ptr(), fx_location.to_raw());
        deref(ptr).ok_or(ReaperFunctionError::new(
            "couldn't get FX GUID (probably FX doesn't exist)",
        ))
    }

    /// Returns the current value of the given track FX in REAPER-normalized form.
    ///
    /// # Errors
    ///
    /// Returns an error if the FX or parameter doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_param_normalized(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        param_index: u32,
    ) -> ReaperFunctionResult<ReaperNormalizedFxParamValue>
    where
        UsageScope: MainThreadOnly,
    {
        let raw_value = self.low.TrackFX_GetParamNormalized(
            track.as_ptr(),
            fx_location.to_raw(),
            param_index as i32,
        );
        if raw_value < 0.0 {
            return Err(ReaperFunctionError::new(
                "couldn't get current FX parameter value (probably FX or parameter doesn't exist)",
            ));
        }
        Ok(ReaperNormalizedFxParamValue::new(raw_value))
    }

    /// Returns the master track of the given project.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn get_master_track(&self, project: ProjectContext) -> MediaTrack
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.get_master_track_unchecked(project) }
    }

    /// Like [`get_master_track()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`get_master_track()`]: #method.get_master_track
    pub unsafe fn get_master_track_unchecked(&self, project: ProjectContext) -> MediaTrack
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.GetMasterTrack(project.to_raw());
        require_non_null_panic(ptr)
    }

    /// Converts the given GUID to a string (including braces).
    pub fn guid_to_string(&self, guid: &GUID) -> CString
    where
        UsageScope: MainThreadOnly,
    {
        let (guid_string, _) = with_string_buffer(64, |buffer, _| unsafe {
            self.low.guidToString(guid as *const GUID, buffer)
        });
        guid_string
    }

    /// Returns the master tempo of the current project.
    pub fn master_get_tempo(&self) -> Bpm
    where
        UsageScope: MainThreadOnly,
    {
        Bpm(self.low.Master_GetTempo())
    }

    /// Sets the current tempo of the given project.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn set_current_bpm(
        &self,
        project: ProjectContext,
        tempo: Bpm,
        undo_behavior: UndoBehavior,
    ) {
        self.require_valid_project(project);
        unsafe {
            self.set_current_bpm_unchecked(project, tempo, undo_behavior);
        }
    }

    /// Like [`set_current_bpm()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`set_current_bpm()`]: #method.set_current_bpm
    pub unsafe fn set_current_bpm_unchecked(
        &self,
        project: ProjectContext,
        tempo: Bpm,
        undo_behavior: UndoBehavior,
    ) {
        self.low.SetCurrentBPM(
            project.to_raw(),
            tempo.get(),
            undo_behavior == UndoBehavior::AddUndoPoint,
        );
    }

    /// Returns the master play rate of the given project.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn master_get_play_rate(&self, project: ProjectContext) -> PlaybackSpeedFactor
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.master_get_play_rate_unchecked(project) }
    }

    /// Like [`master_get_play_rate()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`master_get_play_rate()`]: #method.master_get_play_rate
    pub unsafe fn master_get_play_rate_unchecked(
        &self,
        project: ProjectContext,
    ) -> PlaybackSpeedFactor
    where
        UsageScope: MainThreadOnly,
    {
        let raw = self.low.Master_GetPlayRate(project.to_raw());
        PlaybackSpeedFactor(raw)
    }

    /// Sets the master play rate of the current project.
    pub fn csurf_on_play_rate_change(&self, play_rate: PlaybackSpeedFactor) {
        self.low.CSurf_OnPlayRateChange(play_rate.get());
    }

    /// Shows a message box to the user.
    ///
    /// Blocks the main thread.
    pub fn show_message_box<'a>(
        &self,
        message: impl Into<ReaperStringArg<'a>>,
        title: impl Into<ReaperStringArg<'a>>,
        r#type: MessageBoxType,
    ) -> MessageBoxResult
    where
        UsageScope: MainThreadOnly,
    {
        let result = unsafe {
            self.low.ShowMessageBox(
                message.into().as_ptr(),
                title.into().as_ptr(),
                r#type.to_raw(),
            )
        };
        MessageBoxResult::try_from_raw(result).expect("unknown message box result")
    }

    /// Parses the given string as GUID.
    ///
    /// # Errors
    ///
    /// Returns an error if the given string is not a valid GUID string.
    pub fn string_to_guid<'a>(
        &self,
        guid_string: impl Into<ReaperStringArg<'a>>,
    ) -> ReaperFunctionResult<GUID>
    where
        UsageScope: MainThreadOnly,
    {
        let mut guid = MaybeUninit::uninit();
        unsafe {
            self.low
                .stringToGuid(guid_string.into().as_ptr(), guid.as_mut_ptr());
        }
        let guid = unsafe { guid.assume_init() };
        if guid == ZERO_GUID {
            return Err(ReaperFunctionError::new("GUID string is invalid"));
        }
        Ok(guid)
    }

    /// Sets the input monitoring mode of the given track.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_on_input_monitoring_change_ex(
        &self,
        track: MediaTrack,
        mode: InputMonitoringMode,
        gang_behavior: GangBehavior,
    ) -> i32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.CSurf_OnInputMonitorChangeEx(
            track.as_ptr(),
            mode.to_raw(),
            gang_behavior == GangBehavior::AllowGang,
        )
    }

    /// Sets a track attribute as numerical value.
    ///
    /// # Errors
    ///
    /// Returns an error if an invalid (e.g. non-numerical) track attribute key is passed.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn set_media_track_info_value(
        &self,
        track: MediaTrack,
        attribute_key: TrackAttributeKey,
        new_value: f64,
    ) -> ReaperFunctionResult<()>
    where
        UsageScope: MainThreadOnly,
    {
        let successful = self.low.SetMediaTrackInfo_Value(
            track.as_ptr(),
            attribute_key.into_raw().as_ptr(),
            new_value,
        );
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't set track attribute (maybe attribute key is invalid)",
            ));
        }
        Ok(())
    }

    /// Stuffs a 3-byte MIDI message into a queue or send it to an external MIDI hardware.
    pub fn stuff_midimessage(&self, target: StuffMidiMessageTarget, message: impl ShortMessage) {
        let bytes = message.to_bytes();
        self.low.StuffMIDIMessage(
            target.to_raw(),
            bytes.0.into(),
            bytes.1.into(),
            bytes.2.into(),
        );
    }

    /// Converts a decibel value into a volume slider value.
    pub fn db2slider(&self, value: Db) -> VolumeSliderValue
    where
        UsageScope: MainThreadOnly,
    {
        VolumeSliderValue(self.low.DB2SLIDER(value.get()))
    }

    /// Converts a volume slider value into a decibel value.
    pub fn slider2db(&self, value: VolumeSliderValue) -> Db
    where
        UsageScope: MainThreadOnly,
    {
        Db(self.low.SLIDER2DB(value.get()))
    }

    /// Returns the given track's volume and pan.
    ///
    /// # Errors
    ///
    /// Returns an error if not successful (unclear when this happens).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_track_ui_vol_pan(
        &self,
        track: MediaTrack,
    ) -> ReaperFunctionResult<VolumeAndPan>
    where
        UsageScope: MainThreadOnly,
    {
        let mut volume = MaybeUninit::uninit();
        let mut pan = MaybeUninit::uninit();
        let successful =
            self.low
                .GetTrackUIVolPan(track.as_ptr(), volume.as_mut_ptr(), pan.as_mut_ptr());
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't get track volume and pan",
            ));
        }
        Ok(VolumeAndPan {
            volume: ReaperVolumeValue::new(volume.assume_init()),
            pan: ReaperPanValue::new(pan.assume_init()),
        })
    }

    /// Sets the given track's volume.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_set_surface_volume(
        &self,
        track: MediaTrack,
        volume: ReaperVolumeValue,
        notification_behavior: NotificationBehavior,
    ) {
        self.low.CSurf_SetSurfaceVolume(
            track.as_ptr(),
            volume.get(),
            notification_behavior.to_raw(),
        );
    }

    /// Sets the given track's volume, also supports relative changes and gang.
    ///
    /// Returns the value that has actually been set. I think this only deviates if 0.0 is sent.
    /// Then it returns a slightly higher value - the one which actually corresponds to -150 dB.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_on_volume_change_ex(
        &self,
        track: MediaTrack,
        value_change: ValueChange<ReaperVolumeValue>,
        gang_behavior: GangBehavior,
    ) -> ReaperVolumeValue
    where
        UsageScope: MainThreadOnly,
    {
        let raw = self.low.CSurf_OnVolumeChangeEx(
            track.as_ptr(),
            value_change.value(),
            value_change.is_relative(),
            gang_behavior == GangBehavior::AllowGang,
        );
        ReaperVolumeValue::new(raw)
    }

    /// Sets the given track's pan.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_set_surface_pan(
        &self,
        track: MediaTrack,
        pan: ReaperPanValue,
        notification_behavior: NotificationBehavior,
    ) {
        self.low
            .CSurf_SetSurfacePan(track.as_ptr(), pan.get(), notification_behavior.to_raw());
    }

    /// Sets the given track's pan. Also supports relative changes and gang.
    ///
    /// Returns the value that has actually been set.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_on_pan_change_ex(
        &self,
        track: MediaTrack,
        value_change: ValueChange<ReaperPanValue>,
        gang_behavior: GangBehavior,
    ) -> ReaperPanValue
    where
        UsageScope: MainThreadOnly,
    {
        let raw = self.low.CSurf_OnPanChangeEx(
            track.as_ptr(),
            value_change.value(),
            value_change.is_relative(),
            gang_behavior == GangBehavior::AllowGang,
        );
        ReaperPanValue::new(raw)
    }

    /// Counts the number of selected tracks in the given project.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn count_selected_tracks_2(
        &self,
        project: ProjectContext,
        master_track_behavior: MasterTrackBehavior,
    ) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe { self.count_selected_tracks_2_unchecked(project, master_track_behavior) }
    }

    /// Like [`count_selected_tracks_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`count_selected_tracks_2()`]: #method.count_selected_tracks_2
    pub unsafe fn count_selected_tracks_2_unchecked(
        &self,
        project: ProjectContext,
        master_track_behavior: MasterTrackBehavior,
    ) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.CountSelectedTracks2(
            project.to_raw(),
            master_track_behavior == MasterTrackBehavior::IncludeMasterTrack,
        ) as u32
    }

    /// Selects or deselects the given track.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn set_track_selected(&self, track: MediaTrack, is_selected: bool) {
        self.low.SetTrackSelected(track.as_ptr(), is_selected);
    }

    /// Returns a selected track from the given project.
    ///
    /// # Panics
    ///
    /// Panics if the given project is not valid anymore.
    pub fn get_selected_track_2(
        &self,
        project: ProjectContext,
        selected_track_index: u32,
        master_track_behavior: MasterTrackBehavior,
    ) -> Option<MediaTrack>
    where
        UsageScope: MainThreadOnly,
    {
        self.require_valid_project(project);
        unsafe {
            self.get_selected_track_2_unchecked(
                project,
                selected_track_index,
                master_track_behavior,
            )
        }
    }

    /// Like [`get_selected_track_2()`] but doesn't check if project is valid.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid project.
    ///
    /// [`get_selected_track_2()`]: #method.get_selected_track_2
    pub unsafe fn get_selected_track_2_unchecked(
        &self,
        project: ProjectContext,
        selected_track_index: u32,
        master_track_behavior: MasterTrackBehavior,
    ) -> Option<MediaTrack>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.GetSelectedTrack2(
            project.to_raw(),
            selected_track_index as i32,
            master_track_behavior == MasterTrackBehavior::IncludeMasterTrack,
        );
        NonNull::new(ptr)
    }

    /// Selects exactly one track and deselects all others.
    ///
    /// If `None` is passed, deselects all tracks.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn set_only_track_selected(&self, track: Option<MediaTrack>) {
        let ptr = match track {
            None => null_mut(),
            Some(t) => t.as_ptr(),
        };
        self.low.SetOnlyTrackSelected(ptr);
    }

    /// Deletes the given track.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn delete_track(&self, track: MediaTrack) {
        self.low.DeleteTrack(track.as_ptr());
    }

    /// Returns the number of sends, receives or hardware outputs of the given track.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_track_num_sends(&self, track: MediaTrack, category: TrackSendCategory) -> u32
    where
        UsageScope: MainThreadOnly,
    {
        self.low.GetTrackNumSends(track.as_ptr(), category.to_raw()) as u32
    }

    // Gets or sets an attributes of a send, receive or hardware output of the given track.
    ///
    /// Returns the current value if `new_value` is `null_mut()`.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track or invalid new value.
    pub unsafe fn get_set_track_send_info(
        &self,
        track: MediaTrack,
        category: TrackSendCategory,
        send_index: u32,
        attribute_key: TrackSendAttributeKey,
        new_value: *mut c_void,
    ) -> *mut c_void
    where
        UsageScope: MainThreadOnly,
    {
        self.low.GetSetTrackSendInfo(
            track.as_ptr(),
            category.to_raw(),
            send_index as i32,
            attribute_key.into_raw().as_ptr(),
            new_value,
        )
    }

    /// Convenience function which returns the destination track (`P_DESTTRACK`) of the given send
    /// or receive.
    ///
    /// # Errors
    ///
    /// Returns an error e.g. if the send or receive doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_track_send_info_desttrack(
        &self,
        track: MediaTrack,
        direction: TrackSendDirection,
        send_index: u32,
    ) -> ReaperFunctionResult<MediaTrack>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.get_set_track_send_info(
            track,
            direction.into(),
            send_index,
            TrackSendAttributeKey::DestTrack,
            null_mut(),
        ) as *mut raw::MediaTrack;
        NonNull::new(ptr).ok_or(ReaperFunctionError::new(
            "couldn't get destination track (maybe send doesn't exist)",
        ))
    }

    /// Returns the RPPXML state of the given track.
    ///
    /// With `buffer_size` you can tell REAPER how many bytes of the chunk you want.
    ///
    /// # Panics
    ///
    /// Panics if the given buffer size is 0.
    ///
    /// # Errors
    ///
    /// Returns an error if not successful (unclear when this happens).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn get_track_state_chunk(
        &self,
        track: MediaTrack,
        buffer_size: u32,
        cache_hint: ChunkCacheHint,
    ) -> ReaperFunctionResult<CString>
    where
        UsageScope: MainThreadOnly,
    {
        assert!(buffer_size > 0);
        let (chunk_content, successful) = with_string_buffer(buffer_size, |buffer, max_size| {
            self.low.GetTrackStateChunk(
                track.as_ptr(),
                buffer,
                max_size,
                cache_hint == ChunkCacheHint::UndoMode,
            )
        });
        if !successful {
            return Err(ReaperFunctionError::new("couldn't get track chunk"));
        }
        Ok(chunk_content)
    }

    /// Creates a send, receive or hardware output for the given track.
    ///
    /// Returns the index of the created send or receive.
    ///
    /// # Errors
    ///
    /// Returns an error if not successful (unclear when this happens).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::{ProjectContext::CurrentProject, SendTarget::HardwareOutput};
    ///
    /// let src_track = reaper.functions().get_track(CurrentProject, 0).ok_or("no tracks")?;
    /// let send_index = unsafe {
    ///     reaper.functions().create_track_send(src_track, HardwareOutput)?;
    /// };
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub unsafe fn create_track_send(
        &self,
        track: MediaTrack,
        target: SendTarget,
    ) -> ReaperFunctionResult<u32>
    where
        UsageScope: MainThreadOnly,
    {
        let result = self.low.CreateTrackSend(track.as_ptr(), target.to_raw());
        if result < 0 {
            return Err(ReaperFunctionError::new("couldn't create track send"));
        }
        Ok(result as u32)
    }

    /// Arms or unarms the given track for recording.
    ///
    /// Seems to return `true` if it was armed and `false` if not.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_on_rec_arm_change_ex(
        &self,
        track: MediaTrack,
        mode: RecordArmMode,
        gang_behavior: GangBehavior,
    ) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.low.CSurf_OnRecArmChangeEx(
            track.as_ptr(),
            mode.to_raw(),
            gang_behavior == GangBehavior::AllowGang,
        )
    }

    /// Sets the RPPXML state of the given track.
    ///
    /// # Errors
    ///
    /// Returns an error if not successful (for example if the given chunk is not accepted).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn set_track_state_chunk<'a>(
        &self,
        track: MediaTrack,
        chunk: impl Into<ReaperStringArg<'a>>,
        cache_hint: ChunkCacheHint,
    ) -> ReaperFunctionResult<()>
    where
        UsageScope: MainThreadOnly,
    {
        let successful = self.low.SetTrackStateChunk(
            track.as_ptr(),
            chunk.into().as_ptr(),
            cache_hint == ChunkCacheHint::UndoMode,
        );
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't set track chunk (maybe chunk was invalid)",
            ));
        }
        Ok(())
    }

    /// Shows or hides an FX user interface.
    pub unsafe fn track_fx_show(&self, track: MediaTrack, instruction: FxShowInstruction) {
        self.low.TrackFX_Show(
            track.as_ptr(),
            instruction.location_to_raw(),
            instruction.instruction_to_raw(),
        );
    }

    /// Returns the floating window handle of the given FX, if there is any.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_floating_window(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
    ) -> Option<Hwnd>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self
            .low
            .TrackFX_GetFloatingWindow(track.as_ptr(), fx_location.to_raw());
        NonNull::new(ptr)
    }

    /// Returns whether the user interface of the given FX is open.
    ///
    /// *Open* means either visible in the FX chain window or visible in a floating window.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_open(&self, track: MediaTrack, fx_location: TrackFxLocation) -> bool
    where
        UsageScope: MainThreadOnly,
    {
        self.low
            .TrackFX_GetOpen(track.as_ptr(), fx_location.to_raw())
    }

    /// Sets the given track send's volume.
    ///
    /// Returns the value that has actually been set. If the send doesn't exist, returns 0.0 (which
    /// can also be a valid value that has been set, so that's not very useful).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_on_send_volume_change(
        &self,
        track: MediaTrack,
        send_index: u32,
        value_change: ValueChange<ReaperVolumeValue>,
    ) -> ReaperVolumeValue
    where
        UsageScope: MainThreadOnly,
    {
        let raw = self.low.CSurf_OnSendVolumeChange(
            track.as_ptr(),
            send_index as i32,
            value_change.value(),
            value_change.is_relative(),
        );
        ReaperVolumeValue::new(raw)
    }

    /// Sets the given track send's pan.
    ///
    /// Returns the value that has actually been set.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn csurf_on_send_pan_change(
        &self,
        track: MediaTrack,
        send_index: u32,
        value_change: ValueChange<ReaperPanValue>,
    ) -> ReaperPanValue
    where
        UsageScope: MainThreadOnly,
    {
        let raw = self.low.CSurf_OnSendPanChange(
            track.as_ptr(),
            send_index as i32,
            value_change.value(),
            value_change.is_relative(),
        );
        ReaperPanValue::new(raw)
    }

    /// Grants temporary access to the name of the action registered under the given command ID
    /// within the specified section.
    ///
    /// Returns `None` if the action doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid section.
    pub unsafe fn kbd_get_text_from_cmd<R>(
        &self,
        command_id: CommandId,
        section: SectionContext,
        use_action_name: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self
            .low
            .kbd_getTextFromCmd(command_id.get(), section.to_raw());
        create_passing_c_str(ptr)
            // Removed action returns empty string for some reason. We want None in this case!
            .filter(|s| s.to_bytes().len() > 0)
            .map(use_action_name)
    }

    /// Returns the current on/off state of a toggleable action.
    ///
    /// Returns `None` if the action doesn't support on/off states (or if the action doesn't exist).
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid section.
    //
    // Option makes more sense than Result here because this function is at the same time the
    // correct function to be used to determine *if* an action reports on/off states. So
    // "action doesn't report on/off states" is a valid result.
    pub unsafe fn get_toggle_command_state_2(
        &self,
        section: SectionContext,
        command_id: CommandId,
    ) -> Option<bool>
    where
        UsageScope: MainThreadOnly,
    {
        let result = self
            .low
            .GetToggleCommandState2(section.to_raw(), command_id.to_raw());
        if result == -1 {
            return None;
        }
        return Some(result != 0);
    }

    /// Grants temporary access to the name of the command registered under the given command ID.
    ///
    /// The string will *not* start with `_` (e.g. it will return `SWS_ABOUT`).
    ///
    /// Returns `None` if the given command ID is a built-in action or if there's no such ID.
    pub fn reverse_named_command_lookup<R>(
        &self,
        command_id: CommandId,
        use_command_name: impl FnOnce(&CStr) -> R,
    ) -> Option<R>
    where
        UsageScope: MainThreadOnly,
    {
        let ptr = self.low.ReverseNamedCommandLookup(command_id.to_raw());
        unsafe { create_passing_c_str(ptr) }.map(use_command_name)
    }

    /// Returns a track send's volume and pan.
    ///
    /// # Errors
    ///
    /// Returns an error if the send doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    // // send_idx>=0 for hw ouputs, >=nb_of_hw_ouputs for sends. See GetTrackReceiveUIVolPan.
    // Returns Err if send not existing
    pub unsafe fn get_track_send_ui_vol_pan(
        &self,
        track: MediaTrack,
        send_index: u32,
    ) -> ReaperFunctionResult<VolumeAndPan>
    where
        UsageScope: MainThreadOnly,
    {
        let mut volume = MaybeUninit::uninit();
        let mut pan = MaybeUninit::uninit();
        let successful = self.low.GetTrackSendUIVolPan(
            track.as_ptr(),
            send_index as i32,
            volume.as_mut_ptr(),
            pan.as_mut_ptr(),
        );
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't get track send volume and pan (probably send doesn't exist)",
            ));
        }
        Ok(VolumeAndPan {
            volume: ReaperVolumeValue::new(volume.assume_init()),
            pan: ReaperPanValue::new(pan.assume_init()),
        })
    }

    /// Returns the index of the currently selected FX preset as well as the total preset count.
    ///
    /// # Errors
    ///
    /// Returns an error e.g. if the FX doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_preset_index(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
    ) -> ReaperFunctionResult<TrackFxGetPresetIndexResult>
    where
        UsageScope: MainThreadOnly,
    {
        let mut num_presets = MaybeUninit::uninit();
        let index = self.low.TrackFX_GetPresetIndex(
            track.as_ptr(),
            fx_location.to_raw(),
            num_presets.as_mut_ptr(),
        );
        if index == -1 {
            return Err(ReaperFunctionError::new(
                "couldn't get FX preset index (maybe FX doesn't exist)",
            ));
        }
        Ok(TrackFxGetPresetIndexResult {
            index: index as u32,
            count: num_presets.assume_init() as u32,
        })
    }

    /// Selects a preset of the given track FX.
    ///
    /// # Errors
    ///
    /// Returns an error e.g. if the FX doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_set_preset_by_index(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        preset: FxPresetRef,
    ) -> ReaperFunctionResult<()>
    where
        UsageScope: MainThreadOnly,
    {
        let successful = self.low.TrackFX_SetPresetByIndex(
            track.as_ptr(),
            fx_location.to_raw(),
            preset.to_raw(),
        );
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't select FX preset (maybe FX doesn't exist)",
            ));
        }
        Ok(())
    }

    /// Navigates within the presets of the given track FX.
    ///
    /// # Errors
    ///
    /// Returns an error e.g. if the FX doesn't exist.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # let reaper = reaper_medium::Reaper::default();
    /// use reaper_medium::ProjectContext::CurrentProject;
    /// use reaper_medium::TrackFxLocation::NormalFxChain;
    ///
    /// let track = reaper.functions().get_track(CurrentProject, 0).ok_or("no tracks")?;
    /// // Navigate 2 presets "up"
    /// unsafe {
    ///     reaper.functions().track_fx_navigate_presets(track, NormalFxChain(0), -2)?
    /// };
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
    pub unsafe fn track_fx_navigate_presets(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        increment: i32,
    ) -> ReaperFunctionResult<()>
    where
        UsageScope: MainThreadOnly,
    {
        let successful =
            self.low
                .TrackFX_NavigatePresets(track.as_ptr(), fx_location.to_raw(), increment);
        if !successful {
            return Err(ReaperFunctionError::new(
                "couldn't navigate FX presets (maybe FX doesn't exist)",
            ));
        }
        Ok(())
    }

    /// Returns information about the currently selected preset of the given FX.
    ///
    /// *Currently selected* means the preset which is currently showing in the REAPER dropdown.
    ///
    /// With `buffer size` you can tell REAPER how many bytes of the preset name you want. If
    /// you are not interested in the preset name at all, pass 0.
    ///
    /// # Safety
    ///
    /// REAPER can crash if you pass an invalid track.
    pub unsafe fn track_fx_get_preset(
        &self,
        track: MediaTrack,
        fx_location: TrackFxLocation,
        buffer_size: u32,
    ) -> TrackFxGetPresetResult
    where
        UsageScope: MainThreadOnly,
    {
        if buffer_size == 0 {
            let state_matches_preset =
                self.low
                    .TrackFX_GetPreset(track.as_ptr(), fx_location.to_raw(), null_mut(), 0);
            TrackFxGetPresetResult {
                state_matches_preset,
                name: None,
            }
        } else {
            let (name, state_matches_preset) =
                with_string_buffer(buffer_size, |buffer, max_size| {
                    self.low.TrackFX_GetPreset(
                        track.as_ptr(),
                        fx_location.to_raw(),
                        buffer,
                        max_size,
                    )
                });
            if name.to_bytes().len() == 0 {
                return TrackFxGetPresetResult {
                    state_matches_preset,
                    name: None,
                };
            }
            TrackFxGetPresetResult {
                state_matches_preset,
                name: Some(name),
            }
        }
    }

    /// Grants temporary access to an already open MIDI input device.
    ///
    /// Returns `None` if the device doesn't exist, is not connected or is not already opened. The
    /// device must be enabled in REAPER's MIDI preferences.
    ///
    /// This function is typically called in the [audio hook]. But it's also okay to call it in a
    /// VST plug-in as long as [`is_in_real_time_audio()`] returns `true`.
    ///
    /// See [audio hook] for an example.
    ///
    /// # Design
    ///
    /// The device is not just returned because then we would have to mark this function as unsafe.
    /// Returning the device would tempt the consumer to cache the pointer somewhere, which is bad
    /// because the MIDI device can appear/disappear anytime and REAPER doesn't notify us about it.
    /// If we would call [`get_read_buf()`] on a cached pointer and the MIDI device is gone, REAPER
    /// would crash.
    ///
    /// Calling this function in every audio hook invocation is fast enough and the official way
    /// to tap MIDI messages directly. Because of that we
    /// [take a closure and pass a reference](https://stackoverflow.com/questions/61106587).
    ///
    /// [audio hook]: struct.Reaper.html#method.audio_reg_hardware_hook_add
    /// [`is_in_real_time_audio()`]: #method.is_in_real_time_audio
    /// [`get_read_buf()`]: struct.MidiInput.html#method.get_read_buf
    pub fn get_midi_input<R>(
        &self,
        device_id: MidiInputDeviceId,
        use_device: impl FnOnce(&MidiInput) -> R,
    ) -> Option<R>
    where
        UsageScope: AudioThreadOnly,
    {
        let ptr = self.low.GetMidiInput(device_id.to_raw());
        if ptr.is_null() {
            return None;
        }
        NonNull::new(ptr).map(|nnp| use_device(&MidiInput(nnp)))
    }

    fn require_valid_project(&self, project: ProjectContext) {
        if let ProjectContext::Proj(p) = project {
            assert!(
                self.validate_ptr_2(CurrentProject, p),
                "ReaProject doesn't exist anymore"
            )
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum GetParameterStepSizesResult {
    /// Normal (non-toggleable) parameter.
    ///
    /// Each of the decimal numbers are > 0.
    Normal {
        normal_step: f64,
        small_step: Option<f64>,
        large_step: Option<f64>,
    },
    /// Toggleable parameter.
    Toggle,
}

/// Each of these values can be negative! They are not normalized.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct GetParamExResult {
    /// Current value.
    pub current_value: f64,
    /// Minimum possible value.
    pub min_value: f64,
    /// Center value.
    pub mid_value: f64,
    /// Maximum possible value.
    pub max_value: f64,
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub struct EnumProjectsResult {
    /// Project pointer.
    pub project: ReaProject,
    /// Path to project file (only if project saved and path requested).
    pub file_path: Option<PathBuf>,
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub struct GetMidiDevNameResult {
    /// Whether the device is currently connected.
    pub is_present: bool,
    /// Name of the device (only if name requested).
    pub name: Option<CString>,
}

#[derive(Clone, PartialEq, Hash, Debug)]
pub struct TrackFxGetPresetResult {
    /// Whether the current state of the FX matches the preset.
    ///
    /// `false` if the current FX parameters do not exactly match the preset (in other words, if
    /// the user loaded the preset but moved the knobs afterwards).
    pub state_matches_preset: bool,
    /// Name of the preset.
    pub name: Option<CString>,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct TrackFxGetPresetIndexResult {
    /// Preset index.
    pub index: u32,
    /// Total number of presets available.
    pub count: u32,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct VolumeAndPan {
    /// Volume.
    pub volume: ReaperVolumeValue,
    /// Pan.
    pub pan: ReaperPanValue,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum GetLastTouchedFxResult {
    /// The last touched FX is a track FX.
    TrackFx {
        /// Track on which the FX is located.
        track_ref: TrackRef,
        /// Location of the FX on that track.
        fx_location: TrackFxLocation,
        /// Index of the last touched parameter.
        param_index: u32,
    },
    /// The last touched FX is a take FX.
    TakeFx {
        /// Index of the track on which the item is located.
        track_index: u32,
        /// Index of the item on that track.
        ///
        /// **Attention:** It's an index, so it's zero-based (the one-based result from the
        /// low-level function has been transformed for more consistency).
        item_index: u32,
        /// Index of the take within the item.
        take_index: u32,
        /// Index of the FX within the take FX chain.
        fx_index: u32,
        /// Index of the last touched parameter.
        param_index: u32,
    },
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum GetFocusedFxResult {
    /// The (last) focused FX is a track FX.
    TrackFx {
        /// Track on which the FX is located.
        track_ref: TrackRef,
        /// Location of the FX on that track.
        fx_location: TrackFxLocation,
    },
    /// The (last) focused FX is a take FX.
    TakeFx {
        /// Index of the track on which the item is located.
        track_index: u32,
        /// Index of the item on that track.
        item_index: u32,
        /// Index of the take within the item.
        take_index: u32,
        /// Index of the FX within the take FX chain.
        fx_index: u32,
    },
}

fn make_some_if_greater_than_zero(value: f64) -> Option<f64> {
    if value <= 0.0 || value.is_nan() {
        return None;
    }
    Some(value)
}

unsafe fn deref<T: Copy>(ptr: *const T) -> Option<T> {
    if ptr.is_null() {
        return None;
    }
    Some(*ptr)
}

unsafe fn deref_as<T: Copy>(ptr: *mut c_void) -> Option<T> {
    deref(ptr as *const T)
}

unsafe fn create_passing_c_str<'a>(ptr: *const c_char) -> Option<&'a CStr> {
    if ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ptr))
}

fn convert_tracknumber_to_track_ref(tracknumber: u32) -> TrackRef {
    if tracknumber == 0 {
        TrackRef::MasterTrack
    } else {
        TrackRef::NormalTrack(tracknumber - 1)
    }
}

fn with_string_buffer<T>(
    max_size: u32,
    fill_buffer: impl FnOnce(*mut c_char, i32) -> T,
) -> (CString, T) {
    let vec: Vec<u8> = vec![1; max_size as usize];
    let c_string = unsafe { CString::from_vec_unchecked(vec) };
    let raw = c_string.into_raw();
    let result = fill_buffer(raw, max_size as i32);
    let string = unsafe { CString::from_raw(raw) };
    (string, result)
}

const ZERO_GUID: GUID = GUID {
    Data1: 0,
    Data2: 0,
    Data3: 0,
    Data4: [0; 8],
};

mod private {
    use crate::{MainThreadScope, RealTimeAudioThreadScope};

    pub trait Sealed {}

    impl Sealed for MainThreadScope {}
    impl Sealed for RealTimeAudioThreadScope {}
}
