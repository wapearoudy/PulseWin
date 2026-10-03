// Prevents an extra console window from appearing alongside the tray app in
// release builds. Debug builds keep the console so `PULSEWIN_DEBUG=1` output
// (and panics) stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args=std::env::args().collect::<Vec<_>>();
    if args.iter().any(|arg|arg=="--json"||arg=="--statusline") {
        #[cfg(windows)] unsafe {
            #[link(name="kernel32")] extern "system" {fn AttachConsole(process_id:u32)->i32;}
            let _=AttachConsole(u32::MAX);
        }
        if let Err(error)=pulsewin_lib::usage_report::print(args.iter().any(|arg|arg=="--statusline")) {
            eprintln!("PulseWin: {error}");std::process::exit(1);
        }
        return;
    }
    pulsewin_lib::run()
}
