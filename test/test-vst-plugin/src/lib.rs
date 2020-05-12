use c_str_macro::c_str;

use reaper_high::{ActionKind, Reaper, ReaperGuard};
use reaper_low::ReaperPluginContext;
use reaper_medium::{
    CommandId, MediumHookPostCommand, MediumOnAudioBuffer, MediumReaperControlSurface,
    OnAudioBufferArgs,
};
use std::panic::RefUnwindSafe;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use vst::plugin::{HostCallback, Info, Plugin};
use vst::plugin_main;

plugin_main!(TestVstPlugin);

#[allow(non_snake_case)]
#[no_mangle]
extern "system" fn DllMain(hinstance: *const u8, _: u32, _: *const u8) -> u32 {
    let bla = 5;
    1
}

#[derive(Default)]
struct TestVstPlugin {
    host: HostCallback,
    reaper: Option<reaper_medium::Reaper>,
    reaper_guard: Option<Arc<ReaperGuard>>,
}

impl Plugin for TestVstPlugin {
    fn new(host: HostCallback) -> Self {
        Self {
            host,
            reaper: None,
            reaper_guard: None,
        }
    }

    fn get_info(&self) -> Info {
        Info {
            name: "reaper-rs test".to_string(),
            unique_id: 8372,
            ..Default::default()
        }
    }

    fn init(&mut self) {
        // self.use_medium_level_reaper();
        self.use_high_level_reaper();
    }
}

struct MyOnAudioBuffer {
    sender: std::sync::mpsc::Sender<String>,
    counter: u64,
}

impl MediumOnAudioBuffer for MyOnAudioBuffer {
    fn call(&mut self, args: OnAudioBufferArgs) {
        if self.counter % 100 == 0 {
            self.sender
                .send(format!(
                    "Counter: {}, Args: {:?}, Channels: {:?}\n",
                    self.counter,
                    args,
                    (args.reg.input_nch(), args.reg.output_nch())
                ))
                .expect("couldn't send console logging message to main thread");
        }
        self.counter += 1;
    }
}

struct MyHookPostCommand;

impl MediumHookPostCommand for MyHookPostCommand {
    fn call(command_id: CommandId, _flag: i32) {
        println!("Command {:?} executed", command_id)
    }
}

#[derive(Debug)]
struct MyControlSurface {
    functions: reaper_medium::ReaperFunctions,
    receiver: Receiver<String>,
}

impl RefUnwindSafe for MyControlSurface {}

impl MediumReaperControlSurface for MyControlSurface {
    fn run(&mut self) {
        for msg in self.receiver.try_iter() {
            self.functions.show_console_msg(msg);
        }
    }

    fn set_track_list_change(&self) {
        println!("Track list changed!")
    }
}

impl TestVstPlugin {
    // Exists for demonstration purposes and quick tests
    #[allow(dead_code)]
    fn use_medium_level_reaper(&mut self) {
        let context = ReaperPluginContext::from_vst_plugin(self.host).unwrap();
        let low = reaper_low::Reaper::load(&context);
        let mut med = reaper_medium::Reaper::new(low);
        {
            let (sender, receiver) = channel::<String>();
            med.functions()
                .show_console_msg("Registering control surface ...");
            med.plugin_register_add_csurf_inst(MyControlSurface {
                functions: med.functions().clone(),
                receiver,
            })
            .expect("couldn't register control surface");
            med.functions().show_console_msg("Registering action ...");
            med.plugin_register_add_hook_post_command::<MyHookPostCommand>()
                .expect("couldn't register hook post command");
            med.audio_reg_hardware_hook_add(MyOnAudioBuffer { sender, counter: 0 })
                .expect("couldn't register audio hook");
        }
        self.reaper = Some(med);
    }

    fn use_high_level_reaper(&mut self) {
        let guard = Reaper::guarded(|| {
            let context = ReaperPluginContext::from_vst_plugin(self.host).unwrap();
            Reaper::setup_with_defaults(&context, "info@helgoboss.org");
            let reaper = Reaper::get();
            reaper.activate();
            reaper.show_console_msg(c_str!("Loaded reaper-rs integration test VST plugin\n"));
            reaper.register_action(
                c_str!("reaperRsVstIntegrationTests"),
                c_str!("reaper-rs VST integration tests"),
                || reaper_test::execute_integration_test(|_| ()),
                ActionKind::NotToggleable,
            );
        });
        self.reaper_guard = Some(guard);
    }
}
