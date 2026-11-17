// SPDX-License-Identifier: MPL-2.0
pub mod api_provider;
#[cfg(any(windows, test))]
mod channel_memory;
pub mod config;
pub mod connection_profile;
mod discord_auth;
mod discord_http;
pub mod i18n;
#[cfg(any(windows, test))]
mod message_time;
pub mod model;
pub mod model_catalog;
mod openai;
mod rpc;
mod sender_label;
mod translation;
pub mod transport;
#[cfg(windows)]
pub mod ui;

// Exercise platform-independent rendering math on the non-Windows build host.
// The Windows UI includes the same source through its own module tree.
#[cfg(all(test, not(windows)))]
#[path = "ui/surface_math.rs"]
mod surface_math;

#[cfg(all(test, not(windows)))]
#[path = "ui/ui_policy.rs"]
mod ui_policy;

#[cfg(all(test, not(windows)))]
#[path = "ui/input_return.rs"]
mod input_return;

#[cfg(all(test, not(windows)))]
#[path = "ui/scroll_math.rs"]
mod scroll_math;

#[cfg(all(test, not(windows)))]
#[path = "ui/chat_text.rs"]
mod chat_text;

#[cfg(all(test, not(windows)))]
#[path = "ui/tab_bar.rs"]
mod tab_bar;
