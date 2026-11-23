use std::io::{self, BufRead, BufReader, Read, Write};

use flate2::bufread::GzDecoder;
use ring::digest::{Context, SHA256};

pub const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
pub const APPLE_WORKSPACE: &str = "/Volumes/My Shared Files/workspace/project";

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

pub fn digest_to(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
) -> io::Result<(String, u64)> {
    let mut hash = Context::new(&SHA256);
    let mut buffer = [0_u8; 65_536];
    let mut bytes = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > limit {
            return Err(io::Error::other(
                "Recovery archive exceeds the supported size.",
            ));
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    Ok((
        hash.finish()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        bytes,
    ))
}

pub struct GzipMembers<R: Read> {
    member: Option<GzDecoder<BufReader<R>>>,
}

impl<R: Read> GzipMembers<R> {
    pub fn new(source: R) -> Self {
        Self {
            member: Some(GzDecoder::new(BufReader::new(source))),
        }
    }
}

impl<R: Read> Read for GzipMembers<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while let Some(member) = &mut self.member {
            let count = member.read(output)?;
            if count > 0 {
                return Ok(count);
            }
            let mut input = self
                .member
                .take()
                .expect("current gzip member")
                .into_inner();
            // BSD tar can pad the compressed stream to a record boundary.
            loop {
                let chunk = input.fill_buf()?;
                if chunk.is_empty() {
                    return Ok(0);
                }
                let skipped = chunk.iter().take_while(|byte| **byte == 0).count();
                let padding_only = skipped == chunk.len();
                input.consume(skipped);
                if !padding_only {
                    self.member = Some(GzDecoder::new(input));
                    break;
                }
            }
        }
        Ok(0)
    }
}

pub fn decode_gzip(source: impl Read, target: &mut impl Write, limit: u64) -> io::Result<()> {
    let copied = io::copy(
        &mut GzipMembers::new(source).take(limit.saturating_add(1)),
        target,
    )?;
    if copied > limit {
        return Err(io::Error::other(
            "Recovery archive exceeds the supported size.",
        ));
    }
    Ok(())
}
