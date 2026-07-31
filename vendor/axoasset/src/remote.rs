//! Remote HTTP operations

use camino::{Utf8Path, Utf8PathBuf};
use std::fs;

use crate::{error::*, SourceFile};

/// An unparsed Url (borrowed)
pub type UrlStr = str;
/// An unparsed Url (owned)
pub type UrlString = String;

/// A client for http file requests
///
/// Note that you can and should freely Clone this, as the Client (and its
/// underlying request pool) will be shared between the Clones.
#[derive(Debug, Clone)]
pub struct AxoClient {
    client: reqwest::Client,
}

impl AxoClient {
    /// Create an AxoClient with the given reqwest::Client
    pub fn with_reqwest(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Loads an asset from a URL and returns a [`RemoteAsset`][] containing its body
    pub async fn load_asset(&self, url: &UrlStr) -> Result<RemoteAsset> {
        let response = self.get(url).await?;
        let filename = filename(url, response.headers())?;
        let bytes = response
            .bytes()
            .await
            .map_err(wrap_reqwest_err(url))?
            .to_vec();
        Ok(RemoteAsset {
            url: url.to_string(),
            contents: bytes,
            filename,
        })
    }

    /// GETs the URL and returns a [`crate::SourceFile`][] containing its body
    pub async fn load_source(&self, url: &UrlStr) -> Result<SourceFile> {
        let text = self.load_string(url).await?;
        Ok(SourceFile::new(url, text))
    }

    /// GETs the URL and returns its body as a `String`
    pub async fn load_string(&self, url: &UrlStr) -> Result<String> {
        let response = self.get(url).await?;
        let text = response.text().await.map_err(wrap_reqwest_err(url))?;
        Ok(text)
    }

    /// GETs the URL and returns its body as a `Vec<u8>`
    pub async fn load_bytes(&self, url: &UrlStr) -> Result<Vec<u8>> {
        let response = self.get(url).await?;
        let bytes = response
            .bytes()
            .await
            .map_err(wrap_reqwest_err(url))?
            .to_vec();
        Ok(bytes)
    }

    /// GETs the URL and write its bytes to the given local file
    pub async fn load_and_write_to_file(
        &self,
        url: &UrlStr,
        dest_file: impl AsRef<Utf8Path>,
    ) -> Result<()> {
        let asset = self.load_asset(url).await?;
        asset.write_to_file(dest_file).await
    }

    /// GETs the URL and write its bytes to the given local dir
    ///
    /// The filename used will be computed from the url/mime, and the resulting
    /// filepath will be returned.
    pub async fn load_and_write_to_dir(
        &self,
        url: &UrlStr,
        dest_dir: impl AsRef<Utf8Path>,
    ) -> Result<Utf8PathBuf> {
        let asset = self.load_asset(url).await?;
        asset.write_to_dir(dest_dir).await
    }

    /// GETs the URL and returns the raw [`reqwest::Response`][]
    pub async fn get(&self, url: &UrlStr) -> Result<reqwest::Response> {
        self.client
            .get(url)
            .send()
            .await
            .map_err(wrap_reqwest_err(url))
    }

    /// HEADs the URL and returns the raw [`reqwest::Response`][]
    pub async fn head(&self, url: &UrlStr) -> Result<reqwest::Response> {
        self.client
            .head(url)
            .send()
            .await
            .map_err(wrap_reqwest_err(url))
    }
}

fn wrap_reqwest_err(url: &UrlStr) -> impl FnOnce(reqwest::Error) -> AxoassetError + '_ {
    |details| AxoassetError::RemoteAssetRequestFailed {
        origin_path: url.to_string(),
        details,
    }
}

/// A remote asset is an asset that is fetched over the network.
#[derive(Debug)]
pub struct RemoteAsset {
    /// A string containing a valid filename and extension. The filename is
    /// determined by the origin path and the content-type headers from the
    /// server response.
    filename: String,
    /// A string containing a http or https URL pointing to the asset. This does
    /// not need to be `https://origin.com/myfile.ext` as filename is determined by
    /// content-type headers in the server response.
    url: UrlString,
    /// The contents of the asset as a vector of bytes
    contents: Vec<u8>,
}

impl RemoteAsset {
    /// Gets the filename of the RemoteAsset
    ///
    /// Filename may be computed based on things like mimetypes, and does not necessarily
    /// reflect the raw URL's paths.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Gets the origin_path of the RemoteAsset (this is an alias for `url`)
    pub fn origin_path(&self) -> &str {
        &self.url
    }

    /// Gets the url of the RemoteAsset
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Gets the bytes of the RemoteAsset
    pub fn as_bytes(&self) -> &[u8] {
        &self.contents
    }

    /// Gets the bytes of the RemoteAsset by-value
    pub fn into_bytes(self) -> Vec<u8> {
        self.contents
    }

    /// Writes an RemoteAsset's bytes to the given local directory
    ///
    /// The filename used will be `RemoteAsset::filename`, and the resulting file
    /// path will be returned.
    pub async fn write_to_dir(&self, dest_dir: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let dest_path = dest_dir.as_ref().join(&self.filename);
        self.write_to_file(&dest_path).await?;
        Ok(dest_path)
    }

    /// Writes the RemoteAsset's bytes to the given local filepath
    ///
    /// Note that unlike [`RemoteAsset::write_to_dir`][] this will ignore
    /// the computed `RemoteAsset::filename`, preferring the one given here.
    pub async fn write_to_file(&self, dest_file: impl AsRef<Utf8Path>) -> Result<()> {
        let dest_path = dest_file.as_ref();
        fs::write(dest_path, &self.contents).map_err(|details| {
            AxoassetError::RemoteAssetWriteFailed {
                origin_url: self.url.clone(),
                dest_path: dest_path.to_owned(),
                details,
            }
        })
    }
}

fn mimetype(headers: &reqwest::header::HeaderMap, origin_url: &UrlStr) -> Result<mime::Mime> {
    match headers.get(reqwest::header::CONTENT_TYPE) {
        Some(content_type) => {
            let mtype: mime::Mime = content_type
                .to_str()
                .map_err(|details| AxoassetError::HeaderParse {
                    origin_path: origin_url.to_string(),
                    details,
                })?
                .parse()
                .map_err(|details| AxoassetError::MimeParse {
                    origin_path: origin_url.to_string(),
                    details,
                })?;
            match mtype.type_() {
                mime::IMAGE => Ok(mtype),
                mime::TEXT => Ok(mtype),
                _ => Err(AxoassetError::RemoteAssetNonImageMimeType {
                    origin_path: origin_url.to_string(),
                }),
            }
        }
        None => Err(AxoassetError::RemoteAssetMissingContentTypeHeader {
            origin_path: origin_url.to_string(),
        }),
    }
}

fn extension(mimetype: mime::Mime, origin_path: &UrlStr) -> Option<String> {
    match mimetype.type_() {
        mime::IMAGE => image_extension(mimetype, origin_path).ok(),
        mime::TEXT => text_extension(mimetype, origin_path).ok(),
        _ => None,
    }
}

fn text_extension(mimetype: mime::Mime, origin_path: &UrlStr) -> Result<String> {
    if let Some(extension) = mimetype.suffix() {
        Ok(extension.to_string())
    } else {
        match mimetype.subtype() {
            mime::PLAIN => Ok("txt".to_string()),
            mime::CSS => Ok("css".to_string()),
            _ => Err(AxoassetError::RemoteAssetMimeTypeNotSupported {
                origin_path: origin_path.to_string(),
                mimetype: mimetype.to_string(),
            }),
        }
    }
}

fn image_extension(mimetype: mime::Mime, origin_path: &UrlStr) -> Result<String> {
    if let Some(img_format) = image::ImageFormat::from_mime_type(&mimetype) {
        let extensions = img_format.extensions_str();
        if !extensions.is_empty() {
            Ok(extensions[0].to_string())
        } else {
            Err(
                AxoassetError::RemoteAssetIndeterminateImageFormatExtension {
                    origin_path: origin_path.to_string(),
                },
            )
        }
    } else {
        Err(AxoassetError::RemoteAssetMimeTypeNotSupported {
            origin_path: origin_path.to_string(),
            mimetype: mimetype.to_string(),
        })
    }
}

// FIXME: https://github.com/axodotdev/axoasset/issues/6
// FIXME: https://github.com/axodotdev/axoasset/issues/9
/// Currently, this function will take an asset's origin path, and attempt
/// to identify if the final segment of the URL is a filename.
///
/// If it does not find a filename it will drop the host from the origin
/// url, slugify the set of the path, and then add an extension based on the
/// Mime type in the associated response headers.
///
/// A large portion of the origin path is preserved in the filename to help
/// avoid name conflicts, but this is a half measure at best and leaves a
/// lot of room for improvement.
pub fn filename(origin_url: &UrlStr, headers: &reqwest::header::HeaderMap) -> Result<String> {
    let mut filestem = url::Url::parse(origin_url)
        .map_err(|details| AxoassetError::UrlParse {
            origin_path: origin_url.to_owned(),
            details,
        })?
        .path()
        .to_string()
        .replace('/', "_");
    filestem.remove(0);
    if filestem.contains('.') {
        Ok(filestem)
    } else if let Ok(mimetype) = mimetype(headers, origin_url) {
        if let Some(extension) = extension(mimetype, origin_url) {
            Ok(format!("{filestem}.{extension}"))
        } else {
            Ok(filestem)
        }
    } else {
        Ok(filestem)
    }
}
