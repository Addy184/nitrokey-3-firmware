//! Implementation of `trussed::Platform` for the board,
//! using the specific implementation of our `crate::traits`.

use core::time::Duration;

use crate::hal::{
    peripherals::rtc::Rtc,
    typestates::init_state,
};
use crate::traits::buttons::{Press, Edge};
use crate::traits::rgb_led::{Intensities, RgbLed};
use trussed::platform::{consent, ui};

// Assuming there will only be one way to
// get user presence, this should be fine.
// Used for Ctaphid.keepalive message status.
static mut WAITING: bool = false;
pub struct UserPresenceStatus {}
impl UserPresenceStatus {
    pub(crate) fn set_waiting(waiting: bool) {
        unsafe { WAITING = waiting };
    }
    pub fn waiting() -> bool {
        unsafe{ WAITING }
    }
}

pub struct UserInterface<BUTTONS, RGB>
where
BUTTONS: Press + Edge,
RGB: RgbLed,
{
    rtc: Rtc<init_state::Enabled>,
    buttons: Option<BUTTONS>,
    rgb: Option<RGB>,
    wink: Option<core::ops::Range<Duration>>,
    provisioner: bool,
}

impl<BUTTONS, RGB> UserInterface<BUTTONS, RGB>
where
BUTTONS: Press + Edge,
RGB: RgbLed,
{
    pub fn new(
        rtc: Rtc<init_state::Enabled>,
        _buttons: Option<BUTTONS>,
        rgb: Option<RGB>,
        provisioner: bool,
    ) -> Self {
        let wink = None;
        #[cfg(not(feature = "no-buttons"))]
        let ui = Self { rtc, buttons: _buttons, rgb, wink, provisioner };
        #[cfg(feature = "no-buttons")]
        let ui = Self { rtc, buttons: None, rgb, wink, provisioner };

        ui
    }
}

// color codes Conor picked
const BLACK: Intensities = Intensities { red: 0, green: 0, blue: 0 };
const RED: Intensities = Intensities { red: u8::MAX, green: 0, blue: 0 };
const GREEN: Intensities = Intensities { red: 0, green: u8::MAX, blue: 0x02 };
#[allow(dead_code)]
const BLUE: Intensities = Intensities { red: 0, green: 0, blue: u8::MAX };
const TEAL: Intensities = Intensities { red: 0, green: u8::MAX, blue: 0x5a };
const ORANGE: Intensities = Intensities { red: u8::MAX, green: 0x7e, blue: 0 };
const WHITE: Intensities = Intensities { red: u8::MAX, green: u8::MAX, blue: u8::MAX };

impl<BUTTONS, RGB> trussed::platform::UserInterface for UserInterface<BUTTONS,RGB>
where
BUTTONS: Press + Edge,
RGB: RgbLed,
{
    fn check_user_presence(&mut self) -> consent::Level {
        match &mut self.buttons {
            Some(buttons) => {

                // important to read state before checking for edge,
                // since reading an edge could clear the state.
                let state = buttons.state();
                UserPresenceStatus::set_waiting(true);
                let press_result = buttons.wait_for_any_new_press();
                UserPresenceStatus::set_waiting(false);
                if press_result.is_ok() {
                    if state.a && state.b {
                        consent::Level::Strong
                    } else {
                        consent::Level::Normal
                    }
                } else {
                    consent::Level::None
                }
            }
            None => {
                // With configured with no buttons, that means Solo is operating
                // in passive NFC mode, which means user tapped to indicate presence.
                consent::Level::Normal
            }
        }
    }

    fn set_status(&mut self, status: ui::Status) {
        if let Some(rgb) = &mut self.rgb {

            match status {
                ui::Status::Idle => {
                    if self.provisioner {
                        // white
                        rgb.set(WHITE.into());
                    } else {
                        // green
                        rgb.set(GREEN.into());
                    }
                },
                ui::Status::Processing => {
                    // teal
                    rgb.set(TEAL.into());
                }
                ui::Status::WaitingForUserPresence => {
                    // orange
                    rgb.set(ORANGE.into());
                },
                ui::Status::Error => {
                    // Red
                    rgb.set(RED.into());
                },
            }

        }

        // Abort winking if the device is no longer idle
        if status != ui::Status::Idle {
            self.wink = None;
        }
    }

    fn refresh(&mut self) {
        if self.rgb.is_none() {
            return;
        }

        if let Some(wink) = self.wink.clone() {
            let time = self.uptime();
            if wink.contains(&time) {
                // 250 ms white, 250 ms off
                let color = if (time - wink.start).as_millis() % 500 < 250 {
                    WHITE
                } else {
                    BLACK
                };
                self.rgb.as_mut().unwrap().set(color.into());
                return;
            } else {
                self.set_status(ui::Status::Idle);
                self.wink = None;
            }
        }
    }

    fn uptime(&mut self) -> Duration {
        self.rtc.uptime()
    }

    fn wink(&mut self, duration: Duration) {
        let time = self.uptime();
        self.wink = Some(time..time + duration);
        self.rgb.as_mut().unwrap().set(WHITE.into());
    }
}
