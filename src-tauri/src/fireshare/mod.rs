//! Uploads to a Fireshare server with an upload token (M10).

pub mod client;
pub mod hash;

use serde::{Deserialize, Serialize};
use specta::Type;

pub use client::Client;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: u32,
    pub name: String,
    /// CSS colour, e.g. `#ff5500`.
    pub color: Option<String>,
}

/// Uploads of this game go to `folder` on the server.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderRule {
    pub folder: String,
    pub game_id: u32,
}

/// What an upload may name, from `/api/upload/token/options`.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FireshareOptions {
    pub default_folder: Option<String>,
    pub games: Vec<Game>,
    pub tags: Vec<Tag>,
    pub folder_rules: Vec<FolderRule>,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UploadMeta {
    pub title: Option<String>,
    pub game_id: Option<u32>,
    pub tag_ids: Vec<u32>,
    /// The folder of the game's folder rule; `None` for the server's default folder.
    pub folder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub video_id: String,
    /// `<server>/w/<video id>`.
    pub url: String,
    /// The server already had this clip; nothing was uploaded.
    pub duplicate: bool,
}

/// The server and whether a token is stored. The token itself never goes to the webview.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FireshareConfig {
    pub url: Option<String>,
    pub has_token: bool,
}
