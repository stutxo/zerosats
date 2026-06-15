use std::{env, fmt::Display, path::Path};

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};

pub(crate) trait WrapDisplay<T> {
    fn wrap_display(self, context: &'static str) -> Result<T>;
}

impl<T, E> WrapDisplay<T> for std::result::Result<T, E>
where
    E: Display,
{
    fn wrap_display(self, context: &'static str) -> Result<T> {
        self.map_err(|error| eyre!("{context}: {error}"))
    }
}

pub(crate) async fn load_mercury(
    settings_file: &Path,
) -> Result<mercuryrustlib::client_config::ClientConfig> {
    let settings_path = settings_file
        .canonicalize()
        .wrap_err_with(|| format!("failed to resolve {}", settings_file.display()))?;
    env::set_var("ML_SETTINGS_FILE", settings_path.as_os_str());
    Ok(mercuryrustlib::client_config::load().await)
}
