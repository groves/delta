use std::env;

const COLORTERM: &str = "COLORTERM";
const BAT_THEME: &str = "BAT_THEME";
const GIT_CONFIG_PARAMETERS: &str = "GIT_CONFIG_PARAMETERS";
const GIT_PREFIX: &str = "GIT_PREFIX";
const DELTA_FEATURES: &str = "DELTA_FEATURES";
const DELTA_NAVIGATE: &str = "DELTA_NAVIGATE";
const DELTA_EXPERIMENTAL_MAX_LINE_DISTANCE_FOR_NAIVELY_PAIRED_LINES: &str =
    "DELTA_EXPERIMENTAL_MAX_LINE_DISTANCE_FOR_NAIVELY_PAIRED_LINES";
#[derive(Default, Clone)]
pub struct DeltaEnv {
    pub bat_theme: Option<String>,
    pub colorterm: Option<String>,
    pub current_dir: Option<std::path::PathBuf>,
    pub experimental_max_line_distance_for_naively_paired_lines: Option<String>,
    pub features: Option<String>,
    pub git_config_parameters: Option<String>,
    pub git_prefix: Option<String>,
    pub hostname: Option<String>,
    pub navigate: Option<String>,
}

impl DeltaEnv {
    /// Create a structure with current environment variable
    pub fn init() -> Self {
        let bat_theme = env::var(BAT_THEME).ok();
        let colorterm = env::var(COLORTERM).ok();
        let experimental_max_line_distance_for_naively_paired_lines =
            env::var(DELTA_EXPERIMENTAL_MAX_LINE_DISTANCE_FOR_NAIVELY_PAIRED_LINES).ok();
        let features = env::var(DELTA_FEATURES).ok();
        let git_config_parameters = env::var(GIT_CONFIG_PARAMETERS).ok();
        let git_prefix = env::var(GIT_PREFIX).ok();
        let hostname = hostname();
        let navigate = env::var(DELTA_NAVIGATE).ok();

        let current_dir = env::current_dir().ok();

        Self {
            bat_theme,
            colorterm,
            current_dir,
            experimental_max_line_distance_for_naively_paired_lines,
            features,
            git_config_parameters,
            git_prefix,
            hostname,
            navigate,
        }
    }
}

fn hostname() -> Option<String> {
    grep_cli::hostname().ok()?.to_str().map(|s| s.to_string())
}

#[cfg(test)]
pub mod tests {
    use super::DeltaEnv;
    use lazy_static::lazy_static;
    use std::env;
    use std::sync::{Arc, Mutex};

    lazy_static! {
        static ref ENV_ACCESS: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
    }

    #[test]
    fn test_env_parsing() {
        let _guard = ENV_ACCESS.lock().unwrap();
        let feature = "Awesome Feature";
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("DELTA_FEATURES", feature) };
        let env = DeltaEnv::init();
        assert_eq!(env.features, Some(feature.into()));
        // otherwise `current_dir` is not used in the test cfg:
        assert_eq!(env.current_dir, env::current_dir().ok());
    }
}
