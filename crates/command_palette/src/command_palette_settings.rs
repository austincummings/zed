use serde::{Deserialize};
use settings::{ModalWidthContent, RegisterSetting, Settings};

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, RegisterSetting)]
pub struct CommandPaletteSettings {
    pub modal_max_width: ModalWidthContent,
}

impl Settings for CommandPaletteSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let command_palette = content.command_palette.as_ref().unwrap();

        Self {
            modal_max_width: command_palette.modal_max_width.unwrap(),
        }
    }
}
