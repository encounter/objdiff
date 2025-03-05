use std::{
    fs::File,
    io::{BufWriter, Write},
    ops::DerefMut,
    path::Path,
};

use anyhow::{Context, Result, bail};
use tracing::info;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Json,
    JsonPretty,
    Proto,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "json-pretty" | "json_pretty" => Ok(Self::JsonPretty),
            "binpb" | "pb" | "proto" | "protobuf" => Ok(Self::Proto),
            _ => bail!("Invalid output format: {}", s),
        }
    }

    pub fn from_option(s: Option<&str>) -> Result<Self> {
        match s {
            Some(s) => Self::from_str(s),
            None => Ok(Self::default()),
        }
    }
}

pub fn write_output<T, P>(input: &T, output: Option<P>, format: OutputFormat) -> Result<()>
where
    T: serde::Serialize + prost::Message,
    P: AsRef<Path>,
{
    match output.as_ref().map(|p| p.as_ref()) {
        Some(output) if output != Path::new("-") => {
            info!("Writing to {}", output.display());
            let file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(output)
                .with_context(|| format!("Failed to create file {}", output.display()))?;
            match format {
                OutputFormat::Json => {
                    let mut output = BufWriter::new(file);
                    serde_json::to_writer(&mut output, input)
                        .context("Failed to write output file")?;
                    output.flush().context("Failed to flush output file")?;
                }
                OutputFormat::JsonPretty => {
                    let mut output = BufWriter::new(file);
                    serde_json::to_writer_pretty(&mut output, input)
                        .context("Failed to write output file")?;
                    output.flush().context("Failed to flush output file")?;
                }
                OutputFormat::Proto => {
                    file.set_len(input.encoded_len() as u64)?;
                    let map = unsafe { memmap2::Mmap::map(&file) }
                        .context("Failed to map output file")?;
                    let mut output = map.make_mut().context("Failed to remap output file")?;
                    input.encode(&mut output.deref_mut()).context("Failed to encode output")?;
                }
            }
        }
        _ => match format {
            OutputFormat::Json => {
                serde_json::to_writer(std::io::stdout(), input)?;
            }
            OutputFormat::JsonPretty => {
                serde_json::to_writer_pretty(std::io::stdout(), input)?;
            }
            OutputFormat::Proto => {
                std::io::stdout().write_all(&input.encode_to_vec())?;
            }
        },
    }
    Ok(())
}
