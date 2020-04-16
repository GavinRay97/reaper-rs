use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::ptr::null_mut;

use crate::low_level::{firewall, raw};

use crate::low_level;
use crate::low_level::get_cpp_control_surface;
use crate::low_level::raw::{audio_hook_register_t, gaccel_register_t, GUID, HWND, UNDO_STATE_ALL};
use crate::medium_level::{
    AllowGang, AutomationMode, ControlSurface, DelegatingControlSurface, EnvChunkName,
    ExtensionType, FxShowFlag, GlobalAutomationOverride, HookCommand, HookPostCommand,
    InputMonitoringMode, IsAdd, IsMove, IsUndoOptional, KbdActionValue, KbdSectionInfo, MediaTrack,
    MessageBoxResult, MessageBoxType, MidiInput, MidiOutput, ProjectRef, ReaProject,
    ReaperControlSurface, ReaperPointer, ReaperStringArg, ReaperVersion, RecArmState, RecFx,
    RecordingInput, RegInstr, Relative, SendOrReceive, StuffMidiMessageTarget, ToggleAction,
    TrackEnvelope, TrackFxAddByNameVariant, TrackFxRef, TrackInfoKey, TrackRef, TrackSendCategory,
    TrackSendInfoKey, UndoFlag, WantDefaults, WantMaster, WantUndo,
};
use enumflags2::BitFlags;
use helgoboss_midi::MidiMessage;
use std::convert::{TryFrom, TryInto};
use std::mem::MaybeUninit;
use std::path::PathBuf;

/// This is the medium-level API access point to all REAPER functions. In order to use it, you first
/// must obtain an instance of this struct by invoking [`new`](struct.Reaper.html#method.new).
///
/// It's always possible that a function from the low-level API is missing in the medium-level one.
/// That's because unlike the low-level API, the medium-level API is hand-written and a perpetual
/// work in progress. If you can't find the function that you need, you can always resort to the
/// low-level API by navigating to [`low`](struct.Reaper.html#structfield.low). Of course you are
/// welcome to contribute to bring the medium-level API on par with the low-level one.  
pub struct Reaper {
    /// Returns the low-level REAPER instance
    pub low: low_level::Reaper,
}

const ZERO_GUID: GUID = GUID {
    Data1: 0,
    Data2: 0,
    Data3: 0,
    Data4: [0; 8],
};

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

impl Reaper {
    /// Creates a new instance by getting hold of a
    /// [`low_level::Reaper`](../../low_level/struct.Reaper.html) instance.
    pub fn new(low: low_level::Reaper) -> Reaper {
        Reaper { low }
    }

    /// Returns the requested project and optionally its file name.
    ///
    /// With `projfn_out_optional_sz` you can tell REAPER how many characters of the file name you
    /// want. If you are not interested in the file name at all, pass 0.
    pub fn enum_projects(
        &self,
        proj_ref: ProjectRef,
        projfn_out_optional_sz: u32,
    ) -> Option<EnumProjectsResult> {
        use ProjectRef::*;
        let idx = match proj_ref {
            Current => -1,
            CurrentlyRendering => 0x40000000,
            TabIndex(i) => i as i32,
        };
        if projfn_out_optional_sz == 0 {
            let ptr = unsafe { self.low.EnumProjects(idx, null_mut(), 0) };
            let project = ReaProject::optional(ptr)?;
            Some(EnumProjectsResult {
                project,
                file_path: None,
            })
        } else {
            let (owned_c_string, ptr) =
                with_string_buffer(projfn_out_optional_sz, |buffer, max_size| unsafe {
                    self.low.EnumProjects(idx, buffer, max_size)
                });
            let project = ReaProject::optional(ptr)?;
            if owned_c_string.to_bytes().len() == 0 {
                return Some(EnumProjectsResult {
                    project,
                    file_path: None,
                });
            }
            let owned_string = owned_c_string
                .into_string()
                .expect("Path contains non-UTF8 characters");
            Some(EnumProjectsResult {
                project,
                file_path: Some(PathBuf::from(owned_string)),
            })
        }
    }

    /// Returns the track at the given index. Set `proj` to `null_mut()` in order to look for tracks
    /// in the current project.
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track(&self, proj: Option<ReaProject>, trackidx: u32) -> Option<MediaTrack> {
        let ptr = unsafe { self.low.GetTrack(option_into(proj), trackidx as i32) };
        MediaTrack::optional(ptr)
    }

    /// Returns `true` if the given pointer is a valid object of the right type in project `proj`
    /// (`proj` is ignored if pointer is itself a project).
    pub fn validate_ptr_2<'a>(
        &self,
        proj: Option<ReaProject>,
        pointer: impl Into<ReaperPointer<'a>>,
    ) -> bool {
        let pointer = pointer.into();
        unsafe {
            self.low.ValidatePtr2(
                option_into(proj),
                pointer.as_void(),
                Cow::from(pointer).as_ptr(),
            )
        }
    }

    /// Shows a message to the user (also useful for debugging). Send "\n" for newline and "" to
    /// clear the console.
    pub fn show_console_msg<'a>(&self, msg: impl Into<ReaperStringArg<'a>>) {
        unsafe { self.low.ShowConsoleMsg(msg.into().as_ptr()) }
    }

    /// Gets or sets track arbitrary attributes. This just delegates to the low-level analog. Using
    /// this function is not fun and requires you to use unsafe code. Consider using one of the
    /// type-safe convenience functions instead. They start with `get_media_track_info_` or
    /// `set_media_track_info_`.
    pub unsafe fn get_set_media_track_info(
        &self,
        tr: MediaTrack,
        parmname: TrackInfoKey,
        set_new_value: *mut c_void,
    ) -> *mut c_void {
        self.low
            .GetSetMediaTrackInfo(tr.into(), Cow::from(parmname).as_ptr(), set_new_value)
    }

    fn get_media_track_info(&self, tr: MediaTrack, parmname: TrackInfoKey) -> *mut c_void {
        unsafe { self.get_set_media_track_info(tr, parmname, null_mut()) }
    }

    /// Convenience function which returns the given track's parent track (`P_PARTRACK`).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_partrack(&self, tr: MediaTrack) -> Option<MediaTrack> {
        let ptr = self.get_media_track_info(tr, TrackInfoKey::P_PARTRACK) as *mut raw::MediaTrack;
        MediaTrack::optional(ptr)
    }

    /// Convenience function which returns the given track's parent project (`P_PROJECT`).
    // In REAPER < 5.95 this returns nullptr
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_project(&self, tr: MediaTrack) -> Option<ReaProject> {
        let ptr = self.get_media_track_info(tr, TrackInfoKey::P_PROJECT) as *mut raw::ReaProject;
        ReaProject::optional(ptr)
    }

    /// Convenience function which let's you use the given track's name (`P_NAME`).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_name<R>(
        &self,
        tr: MediaTrack,
        f: impl Fn(&CStr) -> R,
    ) -> Option<R> {
        let ptr = self.get_media_track_info(tr, TrackInfoKey::P_NAME);
        unsafe { create_passing_c_str(ptr as *const c_char) }.map(f)
    }

    /// Convenience function which returns the given track's input monitoring mode (I_RECMON).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_recmon(&self, tr: MediaTrack) -> InputMonitoringMode {
        let ptr = self.get_media_track_info(tr, TrackInfoKey::I_RECMON);
        let irecmon = unsafe { unref_as::<i32>(ptr) }.unwrap();
        InputMonitoringMode::try_from(irecmon).expect("Unknown input monitoring mode")
    }

    /// Convenience function which returns the given track's recording input (I_RECINPUT).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_recinput(&self, tr: MediaTrack) -> Option<RecordingInput> {
        let ptr = self.get_media_track_info(tr, TrackInfoKey::I_RECINPUT);
        let rec_input_index = unsafe { unref_as::<i32>(ptr) }.unwrap();
        if rec_input_index < 0 {
            None
        } else {
            Some(RecordingInput::try_from(rec_input_index as u32).unwrap())
        }
    }

    /// Convenience function which returns the given track's number (IP_TRACKNUMBER).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_tracknumber(&self, tr: MediaTrack) -> Option<TrackRef> {
        use TrackRef::*;
        match self.get_media_track_info(tr, TrackInfoKey::IP_TRACKNUMBER) as i32 {
            -1 => Some(MasterTrack),
            0 => None,
            n if n > 0 => Some(TrackIndex(n as u32 - 1)),
            _ => unreachable!(),
        }
    }

    /// Convenience function which returns the given track's GUID (GUID).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_guid(&self, tr: MediaTrack) -> GUID {
        let ptr = self.get_media_track_info(tr, TrackInfoKey::GUID);
        unsafe { unref_as::<GUID>(ptr) }.unwrap()
    }

    // TODO-doc
    // Kept return value type i32 because meaning of return value depends very much on the actual
    // thing which is registered and probably is not safe to generalize.
    pub unsafe fn plugin_register(&self, name: RegInstr, infostruct: *mut c_void) -> i32 {
        self.low
            .plugin_register(Cow::from(name).as_ptr(), infostruct)
    }

    // TODO-doc
    pub fn plugin_register_hookcommand<T: HookCommand>(&self) -> Result<(), ()> {
        let result = unsafe {
            self.plugin_register(
                RegInstr::Register(ExtensionType::HookCommand),
                delegating_hook_command::<T> as *mut c_void,
            )
        };
        ok_if_one(result)
    }

    // TODO-doc
    pub fn plugin_unregister_hookcommand<T: HookCommand>(&self) {
        unsafe {
            self.plugin_register(
                RegInstr::Unregister(ExtensionType::HookCommand),
                delegating_hook_command::<T> as *mut c_void,
            );
        }
    }

    // TODO-doc
    pub fn plugin_register_toggleaction<T: ToggleAction>(&self) -> Result<(), ()> {
        let result = unsafe {
            self.plugin_register(
                RegInstr::Register(ExtensionType::ToggleAction),
                delegating_toggle_action::<T> as *mut c_void,
            )
        };
        ok_if_one(result)
    }

    // TODO-doc
    pub fn plugin_unregister_toggleaction<T: ToggleAction>(&self) {
        unsafe {
            self.plugin_register(
                RegInstr::Unregister(ExtensionType::ToggleAction),
                delegating_toggle_action::<T> as *mut c_void,
            );
        }
    }

    // TODO-doc
    pub fn plugin_register_hookpostcommand<T: HookPostCommand>(&self) -> Result<(), ()> {
        let result = unsafe {
            self.plugin_register(
                RegInstr::Register(ExtensionType::HookPostCommand),
                delegating_hook_post_command::<T> as *mut c_void,
            )
        };
        ok_if_one(result)
    }

    // TODO-doc
    pub fn plugin_unregister_hookpostcommand<T: HookPostCommand>(&self) {
        unsafe {
            self.plugin_register(
                RegInstr::Unregister(ExtensionType::HookPostCommand),
                delegating_hook_post_command::<T> as *mut c_void,
            );
        }
    }

    // Returns the assigned command index.
    // If the command ID is already used, it just returns the index which has been assigned before.
    // Passing an empty string actually works (!). If a null pointer is passed, 0 is returned, but
    // we can't do that using this signature. If a very large string is passed, it works. If a
    // number of a built-in command is passed, it works.
    // TODO-doc
    pub fn plugin_register_command_id<'a>(
        &self,
        command_id: impl Into<ReaperStringArg<'a>>,
    ) -> u32 {
        unsafe {
            self.plugin_register(
                RegInstr::Register(ExtensionType::CommandId),
                command_id.into().as_ptr() as *mut c_void,
            ) as u32
        }
    }

    // TODO-doc
    // A reference is in line here (vs. pointer) because gaccel_register_t is a struct created on
    // our (Rust) side. It doesn't necessary have to be static because we might just write a
    // script which registers something only shortly and unregisters it again later.
    //
    // gaccel_register_t and similar structs registered with plugin_register cannot be,
    // lifted to medium-level API style. Because at the end of the day
    // REAPER *needs* the correct struct here. Also, with structs we can't do any indirection as
    // with function calls. So at a maxium we can provide some optionally usable
    // factory method for creating such structs. The consumer must ensure that it lives long
    // enough!
    //
    // Unsfe because consumer must ensure proper lifetime of given reference.
    // TODO-low Add factory functions for gaccel_register_t
    pub unsafe fn plugin_register_gaccel(&self, gaccel: &mut gaccel_register_t) -> Result<(), ()> {
        let result = self.plugin_register(
            RegInstr::Register(ExtensionType::GAccel),
            gaccel as *mut _ as *mut c_void,
        );
        ok_if_one(result)
    }

    // TODO-doc
    pub fn plugin_unregister_gaccel(&self, gaccel: &mut gaccel_register_t) {
        unsafe {
            self.plugin_register(
                RegInstr::Unregister(ExtensionType::GAccel),
                gaccel as *mut _ as *mut c_void,
            );
        }
    }

    // TODO-doc
    pub unsafe fn plugin_register_csurf_inst(
        &self,
        csurf_inst: ReaperControlSurface,
    ) -> Result<(), ()> {
        let result = unsafe {
            self.plugin_register(
                RegInstr::Register(ExtensionType::CSurfInst),
                csurf_inst.into(),
            )
        };
        ok_if_one(result)
    }

    // TODO-doc
    pub fn plugin_unregister_csurf_inst(&self, csurf_inst: ReaperControlSurface) {
        unsafe {
            self.plugin_register(
                RegInstr::Unregister(ExtensionType::CSurfInst),
                csurf_inst.into(),
            );
        }
    }

    /// Performs an action belonging to the main action section. To perform non-native actions
    /// (ReaScripts, custom or extension plugins' actions) safely, see
    /// [`named_command_lookup`](struct.Reaper.html#method.named_command_lookup).
    // TODO-high-maybe-invalid-ptr-safe
    pub fn main_on_command_ex(&self, command: u32, flag: i32, proj: Option<ReaProject>) {
        unsafe {
            self.low
                .Main_OnCommandEx(command as i32, flag, option_into(proj));
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_set_surface_mute(
        &self,
        trackid: MediaTrack,
        mute: bool,
        ignoresurf: Option<ReaperControlSurface>,
    ) {
        unsafe {
            self.low
                .CSurf_SetSurfaceMute(trackid.into(), mute, option_into(ignoresurf));
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_set_surface_solo(
        &self,
        trackid: MediaTrack,
        solo: bool,
        ignoresurf: Option<ReaperControlSurface>,
    ) {
        unsafe {
            self.low
                .CSurf_SetSurfaceSolo(trackid.into(), solo, option_into(ignoresurf));
        }
    }

    /// Generates a random GUID.
    pub fn gen_guid(&self) -> GUID {
        let mut guid = MaybeUninit::uninit();
        unsafe {
            self.low.genGuid(guid.as_mut_ptr());
        }
        unsafe { guid.assume_init() }
    }

    // TODO-doc
    // This method is not idempotent. If you call it two times, you will have every callback TWICE.
    // Please take care of unregistering once you are done!
    pub fn register_control_surface(&self) -> Result<(), ()> {
        unsafe {
            self.plugin_register_csurf_inst(ReaperControlSurface::required_panic(
                get_cpp_control_surface() as *mut _,
            ))
        }
    }

    // TODO-doc
    // This method is idempotent
    pub fn unregister_control_surface(&self) {
        self.plugin_unregister_csurf_inst(ReaperControlSurface::required_panic(
            get_cpp_control_surface() as *mut _,
        ));
    }

    // TODO-doc
    pub fn section_from_unique_id(&self, unique_id: u32) -> Option<KbdSectionInfo> {
        let ptr = self.low.SectionFromUniqueID(unique_id as i32);
        KbdSectionInfo::optional(ptr)
    }

    // TODO-doc
    // Kept return value type i32 because I have no idea what the return value is about.
    // TODO-high-maybe-invalid-ptr-safe
    pub fn kbd_on_main_action_ex(
        &self,
        cmd: u32,
        value: KbdActionValue,
        hwnd: HWND,
        proj: Option<ReaProject>,
    ) -> i32 {
        use KbdActionValue::*;
        let (val, valhw, relmode) = match value {
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
        unsafe {
            self.low
                .KBD_OnMainActionEx(cmd as i32, val, valhw, relmode, hwnd, option_into(proj))
        }
    }

    /// Returns the REAPER main window handle.
    pub fn get_main_hwnd(&self) -> HWND {
        self.low.GetMainHwnd()
    }

    // TODO-doc
    pub fn named_command_lookup<'a>(&self, command_name: impl Into<ReaperStringArg<'a>>) -> u32 {
        unsafe { self.low.NamedCommandLookup(command_name.into().as_ptr()) as u32 }
    }

    /// Clears the ReaScript console.
    pub fn clear_console(&self) {
        self.low.ClearConsole();
    }

    /// Returns the number of tracks in the given project (pass `null_mut()` for current project)
    // TODO-high-maybe-invalid-ptr-safe
    pub fn count_tracks(&self, proj: Option<ReaProject>) -> u32 {
        unsafe { self.low.CountTracks(option_into(proj)) as u32 }
    }

    // TODO-doc
    pub fn insert_track_at_index(&self, idx: u32, want_defaults: WantDefaults) {
        self.low
            .InsertTrackAtIndex(idx as i32, want_defaults.into());
    }

    // TODO-doc
    pub fn get_midi_input(&self, idx: u32) -> Option<MidiInput> {
        let ptr = self.low.GetMidiInput(idx as i32);
        MidiInput::optional(ptr)
    }

    // TODO-doc
    pub fn get_midi_output(&self, idx: u32) -> Option<MidiOutput> {
        let ptr = self.low.GetMidiOutput(idx as i32);
        MidiOutput::optional(ptr)
    }

    // TODO-doc
    pub fn get_max_midi_inputs(&self) -> u32 {
        self.low.GetMaxMidiInputs() as u32
    }

    // TODO-doc
    pub fn get_max_midi_outputs(&self) -> u32 {
        self.low.GetMaxMidiOutputs() as u32
    }

    // TODO-doc
    pub fn get_midi_input_name(&self, dev: u32, nameout_sz: u32) -> GetMidiDevNameResult {
        if nameout_sz == 0 {
            let is_present = unsafe { self.low.GetMIDIInputName(dev as i32, null_mut(), 0) };
            GetMidiDevNameResult {
                is_present,
                name: None,
            }
        } else {
            let (name, is_present) = with_string_buffer(nameout_sz, |buffer, max_size| unsafe {
                self.low.GetMIDIInputName(dev as i32, buffer, max_size)
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

    // TODO-doc
    // Return type Option or Result can't be easily chosen here because if instantiate is 0, it
    // should be Option, if it's -1 or > 0, it should be Result. So we just keep the i32.
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_add_by_name<'a>(
        &self,
        track: MediaTrack,
        fxname: impl Into<ReaperStringArg<'a>>,
        rec_fx: RecFx,
        instantiate: TrackFxAddByNameVariant,
    ) -> i32 {
        unsafe {
            self.low.TrackFX_AddByName(
                track.into(),
                fxname.into().as_ptr(),
                rec_fx.into(),
                instantiate.into(),
            )
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_add_by_name_query<'a>(
        &self,
        track: MediaTrack,
        fxname: impl Into<ReaperStringArg<'a>>,
        rec_fx: RecFx,
    ) -> Option<u32> {
        match self.track_fx_add_by_name(track, fxname, rec_fx, TrackFxAddByNameVariant::Query) {
            -1 => None,
            idx if idx >= 0 => Some(idx as u32),
            _ => unreachable!(),
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_add_by_name_add<'a>(
        &self,
        track: MediaTrack,
        fxname: impl Into<ReaperStringArg<'a>>,
        rec_fx: RecFx,
        force_add: bool,
    ) -> Result<u32, ()> {
        match self.track_fx_add_by_name(
            track,
            fxname,
            rec_fx,
            if force_add {
                TrackFxAddByNameVariant::Add
            } else {
                TrackFxAddByNameVariant::AddIfNotFound
            },
        ) {
            -1 => Err(()),
            idx if idx >= 0 => Ok(idx as u32),
            _ => unreachable!(),
        }
    }

    // TODO-doc
    pub fn get_midi_output_name(&self, dev: u32, nameout_sz: u32) -> GetMidiDevNameResult {
        if nameout_sz == 0 {
            let is_present = unsafe { self.low.GetMIDIOutputName(dev as i32, null_mut(), 0) };
            GetMidiDevNameResult {
                is_present,
                name: None,
            }
        } else {
            let (name, is_present) = with_string_buffer(nameout_sz, |buffer, max_size| unsafe {
                self.low.GetMIDIOutputName(dev as i32, buffer, max_size)
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

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_enabled(&self, track: MediaTrack, fx: TrackFxRef) -> bool {
        unsafe { self.low.TrackFX_GetEnabled(track.into(), fx.into()) }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns Err if FX doesn't exist
    pub fn track_fx_get_fx_name(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        buf_sz: u32,
    ) -> Result<CString, ()> {
        assert!(buf_sz > 0);
        let (name, successful) = with_string_buffer(buf_sz, |buffer, max_size| unsafe {
            self.low
                .TrackFX_GetFXName(track.into(), fx.into(), buffer, max_size)
        });
        if !successful {
            return Err(());
        }
        Ok(name)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_instrument(&self, track: MediaTrack) -> Option<u32> {
        let index = unsafe { self.low.TrackFX_GetInstrument(track.into()) };
        if index == -1 {
            return None;
        }
        Some(index as u32)
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_set_enabled(&self, track: MediaTrack, fx: TrackFxRef, enabled: bool) {
        unsafe {
            self.low
                .TrackFX_SetEnabled(track.into(), fx.into(), enabled);
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_num_params(&self, track: MediaTrack, fx: TrackFxRef) -> u32 {
        unsafe { self.low.TrackFX_GetNumParams(track.into(), fx.into()) as u32 }
    }

    // TODO-doc
    pub fn get_current_project_in_load_save(&self) -> Option<ReaProject> {
        let ptr = self.low.GetCurrentProjectInLoadSave();
        ReaProject::optional(ptr)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns Err if FX or parameter doesn't exist
    pub fn track_fx_get_param_name(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
        buf_sz: u32,
    ) -> Result<CString, ()> {
        assert!(buf_sz > 0);
        let (name, successful) = with_string_buffer(buf_sz, |buffer, max_size| unsafe {
            self.low
                .TrackFX_GetParamName(track.into(), fx.into(), param as i32, buffer, max_size)
        });
        if !successful {
            return Err(());
        }
        Ok(name)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns Err if FX or parameter doesn't exist
    pub fn track_fx_get_formatted_param_value(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
        buf_sz: u32,
    ) -> Result<CString, ()> {
        assert!(buf_sz > 0);
        let (name, successful) = with_string_buffer(buf_sz, |buffer, max_size| unsafe {
            self.low.TrackFX_GetFormattedParamValue(
                track.into(),
                fx.into(),
                param as i32,
                buffer,
                max_size,
            )
        });
        if !successful {
            return Err(());
        }
        Ok(name)
    }

    // TODO-doc
    // Returns Err if FX or parameter doesn't exist or if FX doesn't support formatting arbitrary
    // parameter values and the given value is not equal to the current one.
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_format_param_value_normalized(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
        value: f64,
        buf_sz: u32,
    ) -> Result<CString, ()> {
        assert!(buf_sz > 0);
        let (name, successful) = with_string_buffer(buf_sz, |buffer, max_size| unsafe {
            self.low.TrackFX_FormatParamValueNormalized(
                track.into(),
                fx.into(),
                param as i32,
                value,
                buffer,
                max_size,
            )
        });
        if !successful {
            return Err(());
        }
        Ok(name)
    }

    // TODO-doc
    // Returns Err if FX or parameter doesn't exist
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_set_param_normalized(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
        value: f64,
    ) -> Result<(), ()> {
        let successful = unsafe {
            self.low
                .TrackFX_SetParamNormalized(track.into(), fx.into(), param as i32, value)
        };
        if !successful {
            return Err(());
        }
        Ok(())
    }

    // TODO-doc
    pub fn get_focused_fx(&self) -> Option<GetFocusedFxResult> {
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
        let fxnumber = unsafe { fxnumber.assume_init() as u32 };
        use GetFocusedFxResult::*;
        match result {
            0 => None,
            1 => Some(TrackFx {
                track_ref: convert_tracknumber_to_track_ref(tracknumber),
                fx_ref: fxnumber.into(),
            }),
            2 => {
                // TODO-low Add test
                Some(ItemFx {
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

    // TODO-doc
    // Returns None if no FX has been touched yet or if the last-touched FX doesn't exist anymore
    pub fn get_last_touched_fx(&self) -> Option<GetLastTouchedFxResult> {
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
        let fxnumber = unsafe { fxnumber.assume_init() as u32 };
        let paramnumber = unsafe { paramnumber.assume_init() as u32 };
        use GetLastTouchedFxResult::*;
        if tracknumber_high_word == 0 {
            Some(TrackFx {
                track_ref: convert_tracknumber_to_track_ref(tracknumber),
                fx_ref: fxnumber.into(),
                // Although the parameter is called paramnumber, it's zero-based (checked)
                param_index: paramnumber,
            })
        } else {
            // TODO-low Add test
            Some(ItemFx {
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

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_copy_to_track(
        &self,
        src_track: MediaTrack,
        src_fx: TrackFxRef,
        dest_track: MediaTrack,
        dest_fx: TrackFxRef,
        is_move: IsMove,
    ) {
        unsafe {
            self.low.TrackFX_CopyToTrack(
                src_track.into(),
                src_fx.into(),
                dest_track.into(),
                dest_fx.into(),
                is_move.into(),
            );
        }
    }

    // TODO-doc
    // Returns Err if FX doesn't exist (maybe also in other cases?)
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_delete(&self, track: MediaTrack, fx: TrackFxRef) -> Result<(), ()> {
        let succesful = unsafe { self.low.TrackFX_Delete(track.into(), fx.into()) };
        if !succesful {
            return Err(());
        }
        Ok(())
    }

    // TODO-doc
    // Returns None if the FX parameter doesn't report step sizes (or if the FX or parameter doesn't
    // exist, but that can be checked before). Option makes more sense than Result here because
    // this function is at the same time the correct function to be used to determine *if* a
    // parameter reports step sizes. So "parameter doesn't report step sizes" is a valid result.
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_parameter_step_sizes(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
    ) -> Option<GetParameterStepSizesResult> {
        let mut step = MaybeUninit::uninit();
        let mut small_step = MaybeUninit::uninit();
        let mut large_step = MaybeUninit::uninit();
        let mut is_toggle = MaybeUninit::uninit();
        let successful = unsafe {
            self.low.TrackFX_GetParameterStepSizes(
                track.into(),
                fx.into(),
                param as i32,
                step.as_mut_ptr(),
                small_step.as_mut_ptr(),
                large_step.as_mut_ptr(),
                is_toggle.as_mut_ptr(),
            )
        };
        if !successful {
            return None;
        }
        let is_toggle = unsafe { is_toggle.assume_init() };
        if is_toggle {
            Some(GetParameterStepSizesResult::Toggle)
        } else {
            Some(GetParameterStepSizesResult::Normal {
                step: unsafe { step.assume_init() },
                small_step: make_some_if_greater_than_zero(unsafe { small_step.assume_init() }),
                large_step: make_some_if_greater_than_zero(unsafe { large_step.assume_init() }),
            })
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_param_ex(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
    ) -> GetParamExResult {
        let mut min_val = MaybeUninit::uninit();
        let mut max_val = MaybeUninit::uninit();
        let mut mid_val = MaybeUninit::uninit();
        let value = unsafe {
            self.low.TrackFX_GetParamEx(
                track.into(),
                fx.into(),
                param as i32,
                min_val.as_mut_ptr(),
                max_val.as_mut_ptr(),
                mid_val.as_mut_ptr(),
            )
        };
        GetParamExResult {
            value,
            min_val: unsafe { min_val.assume_init() },
            mid_val: unsafe { mid_val.assume_init() },
            max_val: unsafe { max_val.assume_init() },
        }
        .into()
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn undo_begin_block_2(&self, proj: Option<ReaProject>) {
        unsafe {
            self.low.Undo_BeginBlock2(option_into(proj));
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn undo_end_block_2<'a>(
        &self,
        proj: Option<ReaProject>,
        descchange: impl Into<ReaperStringArg<'a>>,
        extraflags: Option<BitFlags<UndoFlag>>,
    ) {
        unsafe {
            self.low.Undo_EndBlock2(
                option_into(proj),
                descchange.into().as_ptr(),
                match extraflags {
                    Some(flags) => flags.bits(),
                    None => UNDO_STATE_ALL,
                } as i32,
            );
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn undo_can_undo_2<R>(
        &self,
        proj: Option<ReaProject>,
        f: impl Fn(&CStr) -> R,
    ) -> Option<R> {
        let ptr = unsafe { self.low.Undo_CanUndo2(option_into(proj)) };
        unsafe { create_passing_c_str(ptr) }.map(f)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn undo_can_redo_2<R>(
        &self,
        proj: Option<ReaProject>,
        f: impl Fn(&CStr) -> R,
    ) -> Option<R> {
        let ptr = unsafe { self.low.Undo_CanRedo2(option_into(proj)) };
        unsafe { create_passing_c_str(ptr) }.map(f)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns true if there was something to be undone, false if not
    pub fn undo_do_undo_2(&self, proj: Option<ReaProject>) -> bool {
        unsafe { self.low.Undo_DoUndo2(option_into(proj)) != 0 }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns true if there was something to be redone, false if not
    pub fn undo_do_redo_2(&self, proj: Option<ReaProject>) -> bool {
        unsafe { self.low.Undo_DoRedo2(option_into(proj)) != 0 }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn mark_project_dirty(&self, proj: Option<ReaProject>) {
        unsafe {
            self.low.MarkProjectDirty(option_into(proj));
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns true if project dirty, false if not
    pub fn is_project_dirty(&self, proj: Option<ReaProject>) -> bool {
        unsafe { self.low.IsProjectDirty(option_into(proj)) != 0 }
    }

    // TODO-doc
    pub fn track_list_update_all_external_surfaces(&self) {
        self.low.TrackList_UpdateAllExternalSurfaces();
    }

    // TODO-doc
    pub fn get_app_version(&self) -> ReaperVersion {
        let ptr = self.low.GetAppVersion();
        let version_str = unsafe { CStr::from_ptr(ptr) };
        version_str.into()
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_automation_mode(&self, tr: MediaTrack) -> AutomationMode {
        let result = unsafe { self.low.GetTrackAutomationMode(tr.into()) };
        AutomationMode::try_from(result).expect("Unknown automation mode")
    }

    // TODO-doc
    pub fn get_global_automation_override(&self) -> Option<GlobalAutomationOverride> {
        use GlobalAutomationOverride::*;
        match self.low.GetGlobalAutomationOverride() {
            -1 => None,
            6 => Some(Bypass),
            x => Some(Mode(x.try_into().expect("Unknown automation mode"))),
        }
    }

    // TODO-doc
    // TODO-low Test
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_envelope_by_chunk_name(
        &self,
        track: MediaTrack,
        cfgchunkname: EnvChunkName,
    ) -> Option<TrackEnvelope> {
        let ptr = unsafe {
            self.low
                .GetTrackEnvelopeByChunkName(track.into(), Cow::from(cfgchunkname).as_ptr())
        };
        TrackEnvelope::optional(ptr)
    }

    // TODO-doc
    // For getting common envelopes (like volume or pan) I recommend using
    // get_track_envelope_by_chunk_name
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_envelope_by_name<'a>(
        &self,
        track: MediaTrack,
        envname: impl Into<ReaperStringArg<'a>>,
    ) -> Option<TrackEnvelope> {
        let ptr = unsafe {
            self.low
                .GetTrackEnvelopeByName(track.into(), envname.into().as_ptr())
        };
        TrackEnvelope::optional(ptr)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_media_track_info_value(&self, tr: MediaTrack, parmname: TrackInfoKey) -> f64 {
        unsafe {
            self.low
                .GetMediaTrackInfo_Value(tr.into(), Cow::from(parmname).as_ptr())
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_count(&self, track: MediaTrack) -> u32 {
        unsafe { self.low.TrackFX_GetCount(track.into()) as u32 }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_rec_count(&self, track: MediaTrack) -> u32 {
        unsafe { self.low.TrackFX_GetRecCount(track.into()) as u32 }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_fx_guid(&self, track: MediaTrack, fx: TrackFxRef) -> Option<GUID> {
        let ptr = unsafe { self.low.TrackFX_GetFXGUID(track.into(), fx.into()) };
        unsafe { unref(ptr) }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_param_normalized(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        param: u32,
    ) -> f64 {
        unsafe {
            self.low
                .TrackFX_GetParamNormalized(track.into(), fx.into(), param as i32)
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_master_track(&self, proj: Option<ReaProject>) -> MediaTrack {
        let ptr = unsafe { self.low.GetMasterTrack(option_into(proj)) };
        MediaTrack::required_panic(ptr)
    }

    // TODO-doc
    pub fn guid_to_string(&self, g: &GUID) -> CString {
        let (guid_string, _) = with_string_buffer(64, |buffer, _| unsafe {
            self.low.guidToString(g as *const GUID, buffer)
        });
        guid_string
    }

    // TODO-doc
    pub fn master_get_tempo(&self) -> f64 {
        self.low.Master_GetTempo()
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn set_current_bpm(&self, proj: Option<ReaProject>, bpm: f64, want_undo: WantUndo) {
        unsafe {
            self.low
                .SetCurrentBPM(option_into(proj), bpm, want_undo.into());
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn master_get_play_rate(&self, project: Option<ReaProject>) -> f64 {
        unsafe { self.low.Master_GetPlayRate(option_into(project)) }
    }

    // TODO-doc
    pub fn csurf_on_play_rate_change(&self, playrate: f64) {
        self.low.CSurf_OnPlayRateChange(playrate);
    }

    // TODO-doc
    pub fn show_message_box<'a>(
        &self,
        msg: impl Into<ReaperStringArg<'a>>,
        title: impl Into<ReaperStringArg<'a>>,
        r#type: MessageBoxType,
    ) -> MessageBoxResult {
        let result = unsafe {
            self.low
                .ShowMessageBox(msg.into().as_ptr(), title.into().as_ptr(), r#type.into())
        };
        result.try_into().expect("Unknown message box result")
    }

    // TODO-doc
    // Returns Err if given string is not a valid GUID string
    pub fn string_to_guid<'a>(&self, str: impl Into<ReaperStringArg<'a>>) -> Result<GUID, ()> {
        let mut guid = MaybeUninit::uninit();
        unsafe {
            self.low
                .stringToGuid(str.into().as_ptr(), guid.as_mut_ptr());
        }
        let guid = unsafe { guid.assume_init() };
        if guid == ZERO_GUID {
            return Err(());
        }
        Ok(guid)
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_on_input_monitoring_change_ex(
        &self,
        trackid: MediaTrack,
        monitor: InputMonitoringMode,
        allowgang: AllowGang,
    ) -> i32 {
        unsafe {
            self.low
                .CSurf_OnInputMonitorChangeEx(trackid.into(), monitor.into(), allowgang.into())
        }
    }

    // TODO-doc
    // Returns Err if invalid parameter name given (maybe also in other situations)
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn set_media_track_info_value(
        &self,
        tr: MediaTrack,
        parmname: TrackInfoKey,
        newvalue: f64,
    ) -> Result<(), ()> {
        let successful = unsafe {
            self.low
                .SetMediaTrackInfo_Value(tr.into(), Cow::from(parmname).as_ptr(), newvalue)
        };
        if !successful {
            return Err(());
        }
        Ok(())
    }

    // TODO-doc
    pub fn stuff_midimessage(&self, mode: StuffMidiMessageTarget, msg: impl MidiMessage) {
        self.low.StuffMIDIMessage(
            mode.into(),
            msg.get_status_byte().into(),
            msg.get_data_byte_1().into(),
            msg.get_data_byte_2().into(),
        );
    }

    // TODO-doc
    pub fn db2slider(&self, x: f64) -> f64 {
        self.low.DB2SLIDER(x)
    }

    // TODO-doc
    pub fn slider2db(&self, y: f64) -> f64 {
        self.low.SLIDER2DB(y)
    }

    // TODO-doc
    // I guess it returns Err if the track doesn't exist
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_ui_vol_pan(&self, track: MediaTrack) -> Result<(f64, f64), ()> {
        let mut volume = MaybeUninit::uninit();
        let mut pan = MaybeUninit::uninit();
        let successful = unsafe {
            self.low
                .GetTrackUIVolPan(track.into(), volume.as_mut_ptr(), pan.as_mut_ptr())
        };
        if !successful {
            return Err(());
        }
        Ok((unsafe { volume.assume_init() }, unsafe {
            pan.assume_init()
        }))
    }

    // TODO-doc
    // TODO-high Same like gaccel
    // Returns true on success
    pub fn audio_reg_hardware_hook(&self, is_add: IsAdd, reg: *mut audio_hook_register_t) -> bool {
        unsafe { self.low.Audio_RegHardwareHook(is_add.into(), reg) > 0 }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_set_surface_volume(
        &self,
        trackid: MediaTrack,
        volume: f64,
        ignoresurf: Option<ReaperControlSurface>,
    ) {
        unsafe {
            self.low
                .CSurf_SetSurfaceVolume(trackid.into(), volume, option_into(ignoresurf));
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_on_volume_change_ex(
        &self,
        trackid: MediaTrack,
        volume: f64,
        relative: Relative,
        allow_gang: AllowGang,
    ) -> f64 {
        unsafe {
            self.low.CSurf_OnVolumeChangeEx(
                trackid.into(),
                volume,
                relative.into(),
                allow_gang.into(),
            )
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_set_surface_pan(
        &self,
        trackid: MediaTrack,
        pan: f64,
        ignoresurf: Option<ReaperControlSurface>,
    ) {
        unsafe {
            self.low
                .CSurf_SetSurfacePan(trackid.into(), pan, option_into(ignoresurf));
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_on_pan_change_ex(
        &self,
        trackid: MediaTrack,
        pan: f64,
        relative: Relative,
        allow_gang: AllowGang,
    ) -> f64 {
        unsafe {
            self.low
                .CSurf_OnPanChangeEx(trackid.into(), pan, relative.into(), allow_gang.into())
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn count_selected_tracks_2(&self, proj: Option<ReaProject>, wantmaster: WantMaster) -> u32 {
        unsafe {
            self.low
                .CountSelectedTracks2(option_into(proj), wantmaster.into()) as u32
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn set_track_selected(&self, track: MediaTrack, selected: bool) {
        unsafe {
            self.low.SetTrackSelected(track.into(), selected);
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_selected_track_2(
        &self,
        proj: Option<ReaProject>,
        seltrackidx: u32,
        wantmaster: WantMaster,
    ) -> Option<MediaTrack> {
        let ptr = unsafe {
            self.low
                .GetSelectedTrack2(option_into(proj), seltrackidx as i32, wantmaster.into())
        };
        MediaTrack::optional(ptr)
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn set_only_track_selected(&self, track: Option<MediaTrack>) {
        unsafe {
            self.low.SetOnlyTrackSelected(option_into(track));
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn delete_track(&self, tr: MediaTrack) {
        unsafe {
            self.low.DeleteTrack(tr.into());
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_num_sends(&self, tr: MediaTrack, category: TrackSendCategory) -> u32 {
        unsafe { self.low.GetTrackNumSends(tr.into(), category.into()) as u32 }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub unsafe fn get_set_track_send_info(
        &self,
        tr: MediaTrack,
        category: TrackSendCategory,
        sendidx: u32,
        parmname: TrackSendInfoKey,
        set_new_value: *mut c_void,
    ) -> *mut c_void {
        self.low.GetSetTrackSendInfo(
            tr.into(),
            category.into(),
            sendidx as i32,
            Cow::from(parmname).as_ptr(),
            set_new_value,
        )
    }

    fn get_track_send_info(
        &self,
        tr: MediaTrack,
        category: TrackSendCategory,
        sendidx: u32,
        parmname: TrackSendInfoKey,
    ) -> *mut c_void {
        unsafe { self.get_set_track_send_info(tr, category, sendidx, parmname, null_mut()) }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_send_info_desttrack(
        &self,
        tr: MediaTrack,
        category: SendOrReceive,
        sendidx: u32,
    ) -> Result<MediaTrack, ()> {
        let ptr =
            self.get_track_send_info(tr, category.into(), sendidx, TrackSendInfoKey::P_DESTTRACK)
                as *mut raw::MediaTrack;
        MediaTrack::required(ptr)
    }

    // TODO-doc
    // I guess it returns Err if the track doesn't exist
    // TODO-high-maybe-invalid-ptr-safe
    pub fn get_track_state_chunk(
        &self,
        track: MediaTrack,
        str_need_big_sz: u32,
        isundo_optional: IsUndoOptional,
    ) -> Result<CString, ()> {
        let (chunk_content, successful) =
            with_string_buffer(str_need_big_sz, |buffer, max_size| unsafe {
                self.low
                    .GetTrackStateChunk(track.into(), buffer, max_size, isundo_optional.into())
            });
        if !successful {
            return Err(());
        }
        Ok(chunk_content)
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn create_track_send(&self, tr: MediaTrack, desttr_in_optional: Option<MediaTrack>) -> u32 {
        unsafe {
            self.low
                .CreateTrackSend(tr.into(), option_into(desttr_in_optional)) as u32
        }
    }

    // TODO-doc
    // Seems to return true if was armed and false if not
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_on_rec_arm_change_ex(
        &self,
        trackid: MediaTrack,
        recarm: RecArmState,
        allowgang: AllowGang,
    ) -> bool {
        unsafe {
            self.low
                .CSurf_OnRecArmChangeEx(trackid.into(), recarm.into(), allowgang.into())
        }
    }

    // TODO-doc
    // Returns Err for example if given chunk is invalid
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn set_track_state_chunk<'a>(
        &self,
        track: MediaTrack,
        str: impl Into<ReaperStringArg<'a>>,
        isundo_optional: IsUndoOptional,
    ) -> Result<(), ()> {
        let successful = unsafe {
            self.low
                .SetTrackStateChunk(track.into(), str.into().as_ptr(), isundo_optional.into())
        };
        if !successful {
            return Err(());
        }
        Ok(())
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn track_fx_show(&self, track: MediaTrack, index: TrackFxRef, show_flag: FxShowFlag) {
        unsafe {
            self.low
                .TrackFX_Show(track.into(), index.into(), show_flag.into());
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_floating_window(&self, track: MediaTrack, index: TrackFxRef) -> HWND {
        unsafe {
            self.low
                .TrackFX_GetFloatingWindow(track.into(), index.into())
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_open(&self, track: MediaTrack, fx: TrackFxRef) -> bool {
        unsafe { self.low.TrackFX_GetOpen(track.into(), fx.into()) }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_on_send_volume_change(
        &self,
        trackid: MediaTrack,
        send_index: u32,
        volume: f64,
        relative: Relative,
    ) -> f64 {
        unsafe {
            self.low.CSurf_OnSendVolumeChange(
                trackid.into(),
                send_index as i32,
                volume,
                relative.into(),
            )
        }
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    pub fn csurf_on_send_pan_change(
        &self,
        trackid: MediaTrack,
        send_index: u32,
        pan: f64,
        relative: Relative,
    ) -> f64 {
        unsafe {
            self.low
                .CSurf_OnSendPanChange(trackid.into(), send_index as i32, pan, relative.into())
        }
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // TODO Check if section can be None
    // Returns None if section or command not existing (can't think of any other case)
    pub fn kbd_get_text_from_cmd<R>(
        &self,
        cmd: u32,
        section: KbdSectionInfo,
        f: impl Fn(&CStr) -> R,
    ) -> Option<R> {
        let ptr = unsafe { self.low.kbd_getTextFromCmd(cmd, section.into()) };
        unsafe { create_passing_c_str(ptr) }
            // Removed action returns empty string for some reason. We want None in this case!
            .filter(|s| s.to_bytes().len() > 0)
            .map(f)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // TODO Check if section can be None
    // Returns None if action doesn't report on/off states (or if action not existing).
    // Option makes more sense than Result here because this function is at the same time the
    // correct function to be used to determine *if* an action reports on/off states. So
    // "action doesn't report on/off states" is a valid result.
    pub fn get_toggle_command_state_2(
        &self,
        section: KbdSectionInfo,
        command_id: u32,
    ) -> Option<bool> {
        let result = unsafe {
            self.low
                .GetToggleCommandState2(section.into(), command_id as i32)
        };
        if result == -1 {
            return None;
        }
        return Some(result != 0);
    }

    // TODO-doc
    // Returns None if lookup was not successful, that is, the command couldn't be found
    pub fn reverse_named_command_lookup<R>(
        &self,
        command_id: u32,
        f: impl Fn(&CStr) -> R,
    ) -> Option<R> {
        let ptr = self.low.ReverseNamedCommandLookup(command_id as i32);
        unsafe { create_passing_c_str(ptr) }.map(f)
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns Err if send not existing
    pub fn get_track_send_ui_vol_pan(
        &self,
        track: MediaTrack,
        send_index: u32,
    ) -> Result<(f64, f64), ()> {
        let mut volume = MaybeUninit::uninit();
        let mut pan = MaybeUninit::uninit();
        let successful = unsafe {
            self.low.GetTrackSendUIVolPan(
                track.into(),
                send_index as i32,
                volume.as_mut_ptr(),
                pan.as_mut_ptr(),
            )
        };
        if !successful {
            return Err(());
        }
        Ok((unsafe { volume.assume_init() }, unsafe {
            pan.assume_init()
        }))
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    // Returns Err e.g. if FX doesn't exist
    pub fn track_fx_get_preset_index(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
    ) -> Result<(u32, u32), ()> {
        let mut num_presets = MaybeUninit::uninit();
        let index = unsafe {
            self.low
                .TrackFX_GetPresetIndex(track.into(), fx.into(), num_presets.as_mut_ptr())
        };
        if index == -1 {
            return Err(());
        }
        return Ok((index as u32, unsafe { num_presets.assume_init() as u32 }));
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    // Returns Err e.g. if FX doesn't exist
    pub fn track_fx_set_preset_by_index(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        idx: i32,
    ) -> Result<(), ()> {
        let successful = unsafe {
            self.low
                .TrackFX_SetPresetByIndex(track.into(), fx.into(), idx)
        };
        if !successful {
            return Err(());
        }
        Ok(())
    }

    // TODO-doc
    // TODO-high-probably-invalid-ptr-unsafe
    // Returns Err e.g. if FX doesn't exist
    pub fn track_fx_navigate_presets(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        presetmove: i32,
    ) -> Result<(), ()> {
        let successful = unsafe {
            self.low
                .TrackFX_NavigatePresets(track.into(), fx.into(), presetmove)
        };
        if !successful {
            return Err(());
        }
        Ok(())
    }

    // TODO-doc
    // TODO-high-maybe-invalid-ptr-safe
    pub fn track_fx_get_preset(
        &self,
        track: MediaTrack,
        fx: TrackFxRef,
        presetname_sz: u32,
    ) -> TrackFxGetPresetResult {
        if presetname_sz == 0 {
            let state_matches_preset = unsafe {
                self.low
                    .TrackFX_GetPreset(track.into(), fx.into(), null_mut(), 0)
            };
            TrackFxGetPresetResult {
                state_matches_preset,
                name: None,
            }
        } else {
            let (name, state_matches_preset) =
                with_string_buffer(presetname_sz, |buffer, max_size| unsafe {
                    self.low
                        .TrackFX_GetPreset(track.into(), fx.into(), buffer, max_size)
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
}

extern "C" fn delegating_hook_command<T: HookCommand>(command_id: i32, flag: i32) -> bool {
    firewall(|| T::call(command_id as u32, flag)).unwrap_or(false)
}

extern "C" fn delegating_toggle_action<T: ToggleAction>(command_id: i32) -> i32 {
    firewall(|| T::call(command_id as u32)).unwrap_or(-1)
}

extern "C" fn delegating_hook_post_command<T: HookPostCommand>(command_id: i32, flag: i32) {
    firewall(|| {
        T::call(command_id as u32, flag);
    });
}

pub enum GetParameterStepSizesResult {
    // Each of the decimal numbers are > 0
    Normal {
        step: f64,
        small_step: Option<f64>,
        large_step: Option<f64>,
    },
    Toggle,
}

// Each of the attributes can be negative! These are not normalized values (0..1).
pub struct GetParamExResult {
    pub value: f64,
    pub min_val: f64,
    pub mid_val: f64,
    pub max_val: f64,
}

pub struct EnumProjectsResult {
    pub project: ReaProject,
    pub file_path: Option<PathBuf>,
}

pub struct GetMidiDevNameResult {
    pub is_present: bool,
    pub name: Option<CString>,
}

pub struct TrackFxGetPresetResult {
    pub state_matches_preset: bool,
    pub name: Option<CString>,
}

pub enum GetLastTouchedFxResult {
    TrackFx {
        track_ref: TrackRef,
        fx_ref: TrackFxRef,
        param_index: u32,
    },
    ItemFx {
        track_index: u32,
        /// **Attention:** It's an index so it's zero-based (the one-based result from the
        /// low-level function is transformed).
        item_index: u32,
        take_index: u32,
        fx_index: u32,
        param_index: u32,
    },
}

pub enum GetFocusedFxResult {
    TrackFx {
        track_ref: TrackRef,
        fx_ref: TrackFxRef,
    },
    ItemFx {
        track_index: u32,
        item_index: u32,
        take_index: u32,
        fx_index: u32,
    },
}

fn make_some_if_greater_than_zero(value: f64) -> Option<f64> {
    if value <= 0.0 || value.is_nan() {
        return None;
    }
    Some(value)
}

unsafe fn unref<T: Copy>(ptr: *const T) -> Option<T> {
    if ptr.is_null() {
        return None;
    }
    Some(*ptr)
}

unsafe fn unref_as<T: Copy>(ptr: *mut c_void) -> Option<T> {
    unref(ptr as *const T)
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
        TrackRef::TrackIndex(tracknumber - 1)
    }
}

fn ok_if_one(result: i32) -> Result<(), ()> {
    if result == 1 { Ok(()) } else { Err(()) }
}

fn option_into<T>(option: Option<impl Into<*mut T>>) -> *mut T {
    match option {
        None => null_mut(),
        Some(v) => v.into(),
    }
}
