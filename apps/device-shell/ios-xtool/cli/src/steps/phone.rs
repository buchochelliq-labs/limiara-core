//! Step 10: the iPhone, passed through from Windows to WSL over USB.

use std::time::Duration;

use super::{Ctx, Status, Step};
use crate::cmd::{Cmd, Input};
use crate::error::{Error, Result};
use crate::parse::{self, UsbDevice};
use crate::validate;
use crate::versions;

pub const PHONE: Step = Step {
    id: "phone",
    title: "iPhone connection",
    why: "WSL does not see USB devices by itself. usbipd-win, on Windows, passes the iPhone through to \
WSL: it is bound once (an administrator action, so Windows asks for permission) and attached each time \
you plug it in. Inside WSL, usbmuxd talks to the phone.",
    check: check_phone,
    run: run_phone,
    when_done: None,
};

fn devices(cx: &mut Ctx) -> Vec<String> {
    cx.run
        .capture(&Cmd::new("idevice_id").arg("-l"))
        .map(|o| parse::idevice_ids(&o.stdout))
        .unwrap_or_default()
}

fn check_phone(cx: &mut Ctx) -> Status {
    match devices(cx).first() {
        Some(udid) => Status::Done(format!("iPhone {udid}")),
        None => Status::Missing("no iPhone is visible in WSL".into()),
    }
}

pub fn usbipd_check(cx: &mut Ctx) -> Status {
    if !cx.run.exists("usbipd.exe") {
        return Status::Missing("usbipd-win is not installed on Windows".into());
    }
    match apple_devices(cx) {
        Ok(list) if list.iter().any(UsbDevice::is_attached) => {
            Status::Done("iPhone attached to WSL".into())
        }
        Ok(list) if list.iter().any(UsbDevice::is_shared) => {
            Status::Missing("iPhone shared, not attached".into())
        }
        Ok(list) if !list.is_empty() => Status::Missing("iPhone connected, not shared yet".into()),
        Ok(_) => Status::Missing("no Apple device on USB".into()),
        Err(_) => Status::Problem("usbipd list failed".into()),
    }
}

pub fn usbmuxd_check(cx: &mut Ctx) -> Status {
    if usbmuxd_running(cx) {
        Status::Done("running".into())
    } else {
        Status::Missing("not running".into())
    }
}

fn usbmuxd_running(cx: &mut Ctx) -> bool {
    cx.run
        .capture(&Cmd::new("pgrep").args(["-x", "usbmuxd"]))
        .is_ok_and(|o| o.success)
}

fn apple_devices(cx: &mut Ctx) -> Result<Vec<UsbDevice>> {
    let out = cx.run.capture(&Cmd::new("usbipd.exe").arg("list"))?;
    if !out.success {
        return Err(Error::CommandFailed {
            program: "usbipd.exe".into(),
            status: "list failed".into(),
        });
    }
    Ok(parse::usbipd_list(&out.stdout)
        .into_iter()
        .filter(UsbDevice::is_apple)
        .collect())
}

/// `usbipd bind` needs administrator rights: started through PowerShell's
/// `Start-Process -Verb RunAs`, so Windows shows its own UAC prompt.
pub fn bind_cmd(bus_id: &str) -> Result<Cmd> {
    if !validate::bus_id(bus_id) {
        return Err(Error::InvalidInput(
            "usbipd printed a bus ID this tool does not understand".into(),
        ));
    }
    Ok(Cmd::new("powershell.exe").args([
        "-NoProfile",
        "-Command",
        &format!("Start-Process -FilePath usbipd -Verb RunAs -Wait -ArgumentList 'bind','--busid','{bus_id}'"),
    ]))
}

pub fn attach_cmd(bus_id: &str) -> Result<Cmd> {
    if !validate::bus_id(bus_id) {
        return Err(Error::InvalidInput(
            "usbipd printed a bus ID this tool does not understand".into(),
        ));
    }
    Ok(Cmd::new("usbipd.exe").args(["attach", "--wsl", "--busid", bus_id]))
}

fn mux_error_note(cx: &mut Ctx) {
    cx.ui.say(&format!(
        "If xtool later fails with [bold]AFCClient.Error.muxError[/], xtool's guide points to a workaround \
that relays the connection from iTunes on Windows instead of usbipd: {}",
        versions::MUX_ERROR_WORKAROUND
    ));
}

fn run_phone(cx: &mut Ctx) -> Result<()> {
    cx.ui
        .say(&format!("Microsoft's guide: {}", versions::USBIPD_GUIDE));
    if !cx.run.exists("usbipd.exe") {
        cx.ui.warn(
            "usbipd-win is missing",
            "Install it on Windows: run [bold]setup-windows.ps1[/] again, or in PowerShell \
[bold]winget install --interactive --exact dorssel.usbipd-win[/]. Then close and reopen this terminal \
and run setup again.",
        );
        mux_error_note(cx);
        return Err(Error::NotReady(
            "usbipd-win is not installed on Windows".into(),
        ));
    }
    let device = loop {
        let list = apple_devices(cx)?;
        match list.len() {
            0 => {
                cx.ui
                    .say("Plug the iPhone in with a USB cable and unlock it.");
                let options = vec!["Look again".to_string(), "Stop".to_string()];
                if cx.ui.choose("No iPhone on USB yet.", &options, 0)? == 1 {
                    return Err(Error::Cancelled);
                }
            }
            1 => break list[0].clone(),
            _ => {
                let options: Vec<String> = list
                    .iter()
                    .map(|d| format!("{} {} ({})", d.bus_id, d.description, d.state))
                    .collect();
                let choice = cx.ui.choose("Which device is the iPhone?", &options, 0)?;
                break list[choice].clone();
            }
        }
    };
    if !device.is_shared() {
        let cmd = bind_cmd(&device.bus_id)?;
        cx.ui.say(
            "Sharing the iPhone with WSL is done once, as administrator. Windows will ask for permission \
(a User Account Control prompt) to run:",
        );
        cx.ui
            .show_command(&Cmd::new("usbipd").args(["bind", "--busid", &device.bus_id]));
        if !cx.ui.confirm("Share it now?", true)? {
            return Err(Error::Cancelled);
        }
        cx.run.interactive(&cmd)?;
    }
    if !device.is_attached() {
        cx.run.interactive(&attach_cmd(&device.bus_id)?)?;
    }
    if !usbmuxd_running(cx) {
        let cmd = Cmd::new("sudo").arg("usbmuxd").input(Input::Inherit);
        if !cx.confirm_sudo(
            std::slice::from_ref(&cmd),
            "starting usbmuxd, which talks to the iPhone",
        )? {
            return Err(Error::Cancelled);
        }
        cx.run.interactive(&cmd)?;
    }
    cx.ui.say(
        "Unlock the iPhone. If it asks [bold]Trust This Computer?[/], tap Trust and enter its passcode \
(xtool shows \"Please tap 'trust' on your device...\" while it waits for that).",
    );
    for _ in 0..15 {
        if !devices(cx).is_empty() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    mux_error_note(cx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_is_elevated_and_bus_ids_are_checked() {
        let cmd = bind_cmd("2-4").unwrap();
        assert_eq!(cmd.program, "powershell.exe");
        assert!(cmd.args[2].contains("-Verb RunAs"));
        assert!(cmd.args[2].contains("'bind','--busid','2-4'"));
        assert!(bind_cmd("2-4'; Remove-Item C:\\ #").is_err());
        assert_eq!(
            attach_cmd("1-1").unwrap().args,
            ["attach", "--wsl", "--busid", "1-1"]
        );
        assert!(attach_cmd("--help").is_err());
    }
}
