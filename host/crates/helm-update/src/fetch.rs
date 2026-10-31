//! Transport abstraction. The library never talks to the network directly;
//! it goes through a [`Fetcher`], so tests can serve fixtures from memory.

use std::io::Read;

use crate::Error;

/// Opens a URL for reading. Implementations must only allow `https://`.
pub trait Fetcher {
    fn open(&self, url: &str, accept: &str) -> Result<Box<dyn Read + Send>, Error>;
}

/// Reads at most `max` bytes; more than that is an error, not a truncation.
pub(crate) fn read_limited(mut r: impl Read, max: u64) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    (&mut r).take(max + 1).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(Error::TooLarge(max));
    }
    Ok(out)
}

pub(crate) fn require_https(url: &str) -> Result<(), Error> {
    if url
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
    {
        Ok(())
    } else {
        Err(Error::InsecureUrl(url.to_string()))
    }
}

#[cfg(feature = "http")]
pub use http::HttpFetcher;

#[cfg(feature = "http")]
mod http {
    use std::io::Read;
    use std::time::Duration;

    use super::{Fetcher, require_https};
    use crate::Error;

    /// Blocking HTTPS fetcher (ureq + rustls), HTTPS-only including redirects.
    pub struct HttpFetcher {
        agent: ureq::Agent,
        user_agent: String,
    }

    impl HttpFetcher {
        pub fn new(user_agent: impl Into<String>) -> Self {
            let agent = ureq::Agent::config_builder()
                .https_only(true)
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_recv_body(Some(Duration::from_secs(300)))
                .build()
                .into();
            Self {
                agent,
                user_agent: user_agent.into(),
            }
        }
    }

    impl Fetcher for HttpFetcher {
        fn open(&self, url: &str, accept: &str) -> Result<Box<dyn Read + Send>, Error> {
            require_https(url)?;
            let resp = self
                .agent
                .get(url)
                .header("User-Agent", &self.user_agent)
                .header("Accept", accept)
                .call()
                .map_err(|e| Error::Http(e.to_string()))?;
            Ok(Box::new(resp.into_body().into_reader()))
        }
    }
}
