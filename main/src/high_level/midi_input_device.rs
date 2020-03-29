use crate::high_level::Reaper;
use std::ffi::CString;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MidiInputDevice {
    id: u32,
}

impl MidiInputDevice {
    pub fn new(id: u32) -> Self {
        assert_ne!(
            id, 63,
            "rec_input_index 63 represents all MIDI input devices"
        );
        MidiInputDevice { id }
    }

    pub fn get_id(&self) -> u32 {
        self.id
    }

    pub fn get_name(&self) -> CString {
        Reaper::get()
            .medium
            .get_midi_input_name(self.id, 33)
            .1
            .into()
    }

    // For REAPER < 5.94 this is the same like isConnected(). For REAPER >=5.94 it returns true if
    // the device ever existed, even if it's disconnected now.
    pub fn is_available(&self) -> bool {
        let (is_present, name) = Reaper::get().medium.get_midi_input_name(self.id, 2);
        is_present || name.to_bytes().len() > 0
    }

    // Only returns true if the device is connected (= present)
    pub fn is_connected(&self) -> bool {
        // In REAPER 5.94 GetMIDIInputName doesn't accept nullptr as name buffer on OS X
        Reaper::get().medium.get_midi_input_name(self.id, 1).0
    }
}
