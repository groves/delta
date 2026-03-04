// Based on code from https://github.com/sharkdp/bat a1b9334a44a2c652f52dddaa83dbacba57372468
// See src/utils/bat/LICENSE

use crate::utils;

pub fn load_highlighting_assets() -> bat::assets::HighlightingAssets {
    bat::assets::HighlightingAssets::from_cache(utils::bat::dirs::PROJECT_DIRS.cache_dir())
        .unwrap_or_else(|_| bat::assets::HighlightingAssets::from_binary())
}
