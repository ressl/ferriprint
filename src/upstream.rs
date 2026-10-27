// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{config::PrinterConfig, domain::ResolvedJob, wire::*};
use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, TryStreamExt};
use ipp::prelude::*;
use std::{path::Path, time::Duration};
use tokio_util::io::{ReaderStream, StreamReader};

#[derive(Clone, Debug)]
pub struct UpstreamStatus {
    pub state: i32,
    pub accepting: bool,
    pub reasons: Vec<String>,
    pub document_formats: Vec<String>,
    pub urf_supported: Vec<String>,
    pub pwg_types: Vec<String>,
}
#[derive(Debug)]
pub struct RemoteJob {
    pub id: i32,
    pub uri: String,
    pub state: i32,
    pub reasons: Vec<String>,
    pub message: String,
    pub impressions: i32,
    pub sheets: i32,
}
#[derive(Debug, thiserror::Error)]
#[error("upstream IPP status {status}: {message}")]
pub struct IppRejection {
    pub status: StatusCode,
    pub job_id: Option<i32>,
    pub message: String,
}
#[derive(Clone)]
pub struct Upstream {
    config: PrinterConfig,
    client: reqwest::Client,
    http_uri: String,
    password: Option<String>,
}
impl Upstream {
    pub fn new(config: &PrinterConfig, timeout: Duration) -> Result<Self> {
        config.validate_transport()?;
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(path) = &config.upstream_ca_certificate {
            let pem = std::fs::read(path).with_context(|| format!("read CA {}", path.display()))?;
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);
        }
        let password = config
            .upstream_auth
            .as_ref()
            .map(|a| {
                std::env::var(&a.password_env).with_context(|| {
                    format!(
                        "missing upstream secret environment variable {}",
                        a.password_env
                    )
                })
            })
            .transpose()?;
        let url = http_url(&config.upstream_uri)?;
        Ok(Self {
            config: config.clone(),
            client: builder.build()?,
            http_uri: url.to_string(),
            password,
        })
    }
    fn request(&self, operation: Operation, id: Option<i32>) -> Result<IppRequestResponse> {
        let mut request = IppRequestResponse::new(
            IppVersion::v2_0(),
            operation,
            Some(self.config.upstream_uri.as_str().parse()?),
        )?;
        if let Some(id) = id {
            add(
                &mut request,
                DelimiterTag::OperationAttributes,
                "job-id",
                IppValue::Integer(id),
            )?;
        }
        add(
            &mut request,
            DelimiterTag::OperationAttributes,
            "requesting-user-name",
            name(&self.config.upstream_username)?,
        )?;
        Ok(request)
    }
    async fn send(
        &self,
        request: IppRequestResponse,
        document: Option<&Path>,
    ) -> Result<IppRequestResponse> {
        let request_id = request.header().request_id;
        let prefix = request.to_bytes();
        let prefix_len = prefix.len() as u64;
        let body = if let Some(path) = document {
            let file = tokio::fs::File::open(path).await?;
            let length = file.metadata().await?.len();
            let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(prefix) })
                .chain(ReaderStream::new(file));
            (reqwest::Body::wrap_stream(stream), prefix_len + length)
        } else {
            (reqwest::Body::from(prefix), prefix_len)
        };
        let mut http = self
            .client
            .post(&self.http_uri)
            .header("content-type", "application/ipp")
            .header("content-length", body.1)
            .body(body.0);
        if let Some(auth) = &self.config.upstream_auth {
            http = http.basic_auth(&auth.username, self.password.as_ref());
        }
        let response = http.send().await?.error_for_status()?;
        ensure!(
            response
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|s| s.split(';').next() == Some("application/ipp")),
            "upstream did not return application/ipp"
        );
        let mut reader = StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
        let response = read_metadata(&mut reader, 256 * 1024).await?;
        ensure!(
            response.header().request_id == request_id,
            "upstream request-id mismatch"
        );
        if !response.header().status_code().is_success() {
            return Err(IppRejection {
                status: response.header().status_code(),
                job_id: attr(&response, DelimiterTag::JobAttributes, "job-id")
                    .and_then(integer)
                    .filter(|id| *id > 0),
                message: attr(
                    &response,
                    DelimiterTag::OperationAttributes,
                    "status-message",
                )
                .and_then(str_value)
                .unwrap_or("request rejected")
                .into(),
            }
            .into());
        }
        Ok(response)
    }
    pub async fn inspect(&self) -> Result<UpstreamStatus> {
        let mut request = self.request(Operation::GetPrinterAttributes, None)?;
        add(
            &mut request,
            DelimiterTag::OperationAttributes,
            "requested-attributes",
            strings(
                &[
                    "printer-state".into(),
                    "printer-is-accepting-jobs".into(),
                    "printer-state-reasons".into(),
                    "document-format-supported".into(),
                    "urf-supported".into(),
                    "pwg-raster-document-type-supported".into(),
                ],
                keyword,
            )?,
        )?;
        let response = self.send(request, None).await?;
        let group = DelimiterTag::PrinterAttributes;
        let state = attr(&response, group, "printer-state")
            .and_then(integer)
            .context("upstream omitted printer-state")?;
        let accepting = matches!(
            attr(&response, group, "printer-is-accepting-jobs"),
            Some(IppValue::Boolean(true))
        );
        let mut document_formats = string_list(attr(&response, group, "document-format-supported"));
        let urf_supported = string_list(attr(&response, group, "urf-supported"));
        let pwg_types = string_list(attr(&response, group, "pwg-raster-document-type-supported"));
        // Raster admission needs actual decoder capabilities, not just an opaque MIME claim.
        document_formats.retain(|f| {
            (f != "image/urf" || !urf_supported.is_empty())
                && (f != "image/pwg-raster" || !pwg_types.is_empty())
        });
        ensure!(
            !document_formats.is_empty(),
            "upstream omitted usable document-format-supported"
        );
        Ok(UpstreamStatus {
            state,
            accepting,
            reasons: string_list(attr(&response, group, "printer-state-reasons")),
            document_formats,
            urf_supported,
            pwg_types,
        })
    }
    pub async fn submit(
        &self,
        title: &str,
        uuid: &str,
        job: &ResolvedJob,
        document: &Path,
    ) -> Result<RemoteJob> {
        let mut request = self.request(Operation::PrintJob, None)?;
        let op = DelimiterTag::OperationAttributes;
        add(&mut request, op, "job-name", name(bounded(title, 255))?)?;
        add(
            &mut request,
            op,
            "document-format",
            mime(&job.document_format)?,
        )?;
        add(
            &mut request,
            op,
            "ipp-attribute-fidelity",
            IppValue::Boolean(true),
        )?;
        let group = DelimiterTag::JobAttributes;
        // job-account-id is a stable operator-visible identifier; local UUID is also persisted.
        add(&mut request, group, "job-account-id", name(uuid)?)?;
        add(&mut request, group, "copies", IppValue::Integer(job.copies))?;
        add(
            &mut request,
            group,
            "orientation-requested",
            IppValue::Enum(job.orientation),
        )?;
        add(
            &mut request,
            group,
            "printer-resolution",
            IppValue::Resolution {
                cross_feed: job.resolution.x,
                feed: job.resolution.y,
                units: 3,
            },
        )?;
        add(
            &mut request,
            group,
            "print-scaling",
            keyword(&job.print_scaling)?,
        )?;
        for (k, v) in &job.options {
            add(&mut request, group, k, name(v)?)?;
        }
        let response = self.send(request, Some(document)).await?;
        let remote = remote_job(&response, None, &self.config)?;
        if response.header().status_code() != StatusCode::SuccessfulOk {
            return Err(IppRejection {status:response.header().status_code(),job_id:Some(remote.id),message:"upstream ignored/substituted attributes despite strict fidelity; canceling known job".into()}.into());
        }
        Ok(remote)
    }
    pub async fn job(&self, id: i32) -> Result<RemoteJob> {
        let request = self.request(Operation::GetJobAttributes, Some(id))?;
        remote_job(&self.send(request, None).await?, Some(id), &self.config)
    }
    pub async fn cancel(&self, id: i32) -> Result<()> {
        self.send(self.request(Operation::CancelJob, Some(id))?, None)
            .await?;
        Ok(())
    }
}
fn remote_job(
    response: &IppRequestResponse,
    known: Option<i32>,
    config: &PrinterConfig,
) -> Result<RemoteJob> {
    let group = DelimiterTag::JobAttributes;
    let id = attr(response, group, "job-id")
        .and_then(integer)
        .or(known)
        .context("upstream omitted job-id; submission outcome ambiguous")?;
    ensure!(id > 0, "invalid upstream job-id");
    let uri = attr(response, group, "job-uri")
        .and_then(str_value)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}#job-{id}", config.upstream_uri));
    let state = attr(response, group, "job-state")
        .and_then(integer)
        .unwrap_or(3);
    ensure!((3..=9).contains(&state), "invalid upstream job-state");
    Ok(RemoteJob {
        id,
        uri,
        state,
        reasons: string_list(attr(response, group, "job-state-reasons")),
        message: attr(response, group, "job-state-message")
            .and_then(str_value)
            .unwrap_or("")
            .into(),
        impressions: attr(response, group, "job-impressions-completed")
            .and_then(integer)
            .unwrap_or(0),
        sheets: attr(response, group, "job-media-sheets-completed")
            .and_then(integer)
            .unwrap_or(0),
    })
}
pub async fn inspect(config: &PrinterConfig, timeout: Duration) -> Result<UpstreamStatus> {
    Upstream::new(config, timeout)?.inspect().await
}
