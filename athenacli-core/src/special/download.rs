//! `download`: fetch the last query's result object from S3 into `/tmp/`.
//! Python shells out to `aws s3 cp`; we use `aws-sdk-s3` directly (plan
//! decision: no shell dependency).

use super::{Emit, Flow, Invocation, Sink, SpecialCtx, SpecialResult};

pub fn download(ctx: &mut SpecialCtx, _inv: &Invocation, sink: &mut Sink) -> anyhow::Result<Flow> {
    let Some(location) = ctx.session.last_output_location.clone() else {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message("No OUTPUT_LOCATION from last query")),
        )?;
        return Ok(Flow::Continue);
    };

    let Some((bucket, key)) = parse_s3_url(&location) else {
        sink(
            ctx.session,
            Emit::Special(SpecialResult::message(format!(
                "Unsupported OUTPUT_LOCATION: {location}"
            ))),
        )?;
        return Ok(Flow::Continue);
    };

    let filename = key.rsplit('/').next().unwrap_or(&key);
    let dest = std::path::Path::new("/tmp").join(filename);
    sink(
        ctx.session,
        Emit::Special(SpecialResult::message(format!(
            "Downloading {location} to {}",
            dest.display()
        ))),
    )?;

    let client = aws_sdk_s3::Client::new(ctx.exec.sdk_config());
    let handle = ctx.exec.handle();
    let bytes = handle.block_on(async {
        let resp = client.get_object().bucket(&bucket).key(&key).send().await?;
        let data = resp.body.collect().await?;
        anyhow::Ok(data.into_bytes())
    })?;
    std::fs::write(&dest, &bytes)?;

    sink(
        ctx.session,
        Emit::Special(SpecialResult::message(format!(
            "Saved {} bytes to {}",
            bytes.len(),
            dest.display()
        ))),
    )?;
    Ok(Flow::Continue)
}

/// `s3://bucket/key/parts` -> (bucket, key).
pub fn parse_s3_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("s3://")?;
    let (bucket, key) = rest.split_once('/')?;
    if bucket.is_empty() || key.is_empty() {
        return None;
    }
    Some((bucket.to_string(), key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_s3_urls() {
        assert_eq!(
            parse_s3_url("s3://bucket/a/b/c.csv"),
            Some(("bucket".into(), "a/b/c.csv".into()))
        );
        assert_eq!(parse_s3_url("s3://bucket/"), None);
        assert_eq!(parse_s3_url("http://x/y"), None);
    }
}
