use std::io;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;

use chrono::prelude::*;
use tokio::sync::mpsc::{self, Sender};

use crate::dns::*;
use crate::infra::mapped_file::MappedFile;
use crate::libdns::proto::op::Query;
use crate::log::warn;
use crate::middleware::*;

pub struct DnsAuditMiddleware {
    audit_sender: Sender<DnsAuditRecord>,
    include_soa: bool,
}

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for DnsAuditMiddleware {
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        let now = Local::now();

        let start = Instant::now();

        let res = next.run(ctx, req).await;

        let duration = start.elapsed();
        if !self.include_soa && is_soa_only(&res, req.query().original()) {
            return res;
        }
        let speed = match res.as_ref().map(|response| response.probe_result()) {
            Ok(ProbeResult::Measured(speed)) => speed,
            _ => ctx.fastest_speed,
        };

        let audit = DnsAuditRecord {
            id: req.id(),
            date: now,
            client: req.src().to_string(),
            query: req.query().original().to_owned(),
            result: res.clone(),
            elapsed: duration,
            speed,
            lookup_source: ctx.source.clone(),
        };

        // debug!("{}", audit.to_string_without_date());

        self.audit_sender
            .send(audit)
            .await
            .unwrap_or_else(|err| warn!("send audit failed,{}", err));

        res
    }
}

impl DnsAuditMiddleware {
    pub fn new(cfg: &crate::dns_conf::RuntimeConfig) -> Self {
        let config = cfg.audit_config();
        let console = config.console.unwrap_or(false);
        let syslog = config.syslog.unwrap_or(false);
        let include_soa = config.soa.unwrap_or(false);
        let path = cfg
            .audit_file()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| cfg.log_file().with_file_name("smartdns-audit.log"));
        let mut file = (!syslog && cfg.audit_num() > 0).then(|| {
            MappedFile::open(
                path,
                cfg.audit_size(),
                Some(cfg.audit_num()),
                Some(cfg.audit_file_mode()),
            )
        });
        let (audit_tx, mut audit_rx) = mpsc::channel::<DnsAuditRecord>(100);
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(10);
            while audit_rx.recv_many(&mut batch, 10).await > 0 {
                for audit in &batch {
                    crate::plugins::audit(&audit.to_string());
                    if console {
                        let _ = writeln!(io::stdout(), "{audit}");
                    }
                    if syslog
                        && let Err(error) = crate::infra::syslog::send(
                            crate::log::Level::INFO,
                            &audit.to_string_without_date(),
                        )
                    {
                        warn!("audit syslog failed: {error}");
                    }
                }
                if let Some(file) = file.as_mut()
                    && let Err(error) = record_audit_to_file(file, &batch)
                {
                    warn!("log audit failed {error}");
                }
                batch.clear();
            }
        });
        Self {
            audit_sender: audit_tx,
            include_soa,
        }
    }
}

fn is_soa_only(result: &Result<DnsResponse, DnsError>, query: &Query) -> bool {
    let response = match result {
        Ok(response) => std::borrow::Cow::Borrowed(response),
        Err(error) => match error.as_no_records_response(query) {
            Some(response) => std::borrow::Cow::Owned(response),
            None => return false,
        },
    };
    response.ip_addrs_iter().next().is_none()
        && response
            .answers()
            .iter()
            .chain(response.authorities())
            .any(|record| record.record_type() == RecordType::SOA)
}

#[derive(Debug, Clone)]
pub struct DnsAuditRecord {
    id: u16,
    client: String,
    query: Query,
    result: Result<DnsResponse, DnsError>,
    speed: Duration,
    elapsed: Duration,
    date: DateTime<Local>,
    lookup_source: LookupFrom,
}

impl DnsAuditRecord {
    fn fmt_result(&self) -> String {
        if let Ok(lookup) = self.result.as_ref() {
            let mut out = String::new();

            for (i, record) in lookup
                .records()
                .iter()
                .chain(lookup.authorities())
                .enumerate()
            {
                let data = record.data();

                if i > 0 {
                    out.push('|');
                }

                out.push_str(data.to_string().as_str());

                out.push(' ');
                out.push_str(record.ttl().to_string().as_str());
                out.push(' ');
                out.push_str(record.record_type().to_string().as_str());
            }
            out
        } else {
            "query failed".to_string()
        }
    }

    fn to_string_without_date(&self) -> String {
        format!(
            "{} query {}, type: {}, elapsed: {:?}, speed: {:?}, result {}",
            self.client,
            self.query.name(),
            self.query.query_type(),
            self.elapsed,
            self.speed,
            self.fmt_result()
        )
    }
}

impl std::fmt::Display for DnsAuditRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} query {}, type: {}, elapsed: {:?}, speed: {:?}, result {}",
            self.date.format("%Y-%m-%d %H:%M:%S,%3f"),
            self.client,
            self.query.name(),
            self.query.query_type(),
            self.elapsed,
            self.speed,
            self.fmt_result()
        )
    }
}

fn record_audit_to_file(
    audit_file: &mut MappedFile,
    audit_records: &[DnsAuditRecord],
) -> io::Result<()> {
    if matches!(audit_file.extension(), Some(ext) if ext == "csv") {
        // write as csv

        if audit_file.peamble().is_none() {
            let mut writer = csv::Writer::from_writer(vec![]);
            writer.write_record([
                "id",
                "timestamp",
                "client",
                "name",
                "type",
                "elapsed",
                "speed",
                "state",
                "result",
                "lookup_source",
            ])?;

            audit_file.set_peamble(Some(
                writer
                    .into_inner()
                    .expect("read csv peamble")
                    .into_boxed_slice(),
            ))
        }

        let mut writer = csv::Writer::from_writer(audit_file);

        for audit in audit_records {
            writer.write_record([
                audit.id.to_string().as_str(),
                audit.date.timestamp().to_string().as_str(),
                audit.client.as_str(),
                audit.query.name().to_string().as_str(),
                audit.query.query_type().to_string().as_str(),
                format!("{:?}", audit.elapsed).as_str(),
                format!("{:?}", audit.speed).as_str(),
                if audit.result.is_ok() {
                    "success"
                } else {
                    "failed"
                },
                audit.fmt_result().as_str(),
                format!("{:?}", audit.lookup_source).as_str(),
            ])?;
        }
    } else {
        // write as nornmal log format.
        for audit in audit_records {
            if writeln!(audit_file, "{audit}").is_err() {
                warn!("Write audit to file '{:?}' failed", audit_file.path());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::libdns::proto::op::Query;
    use crate::libdns::proto::rr::{RData, RecordType};
    use std::io::Read;
    use std::str::FromStr;

    use super::*;

    #[tokio::test]
    async fn test_single_audit_is_written_without_waiting_for_ten_queries() {
        let file =
            std::env::temp_dir().join(format!("smartdns-audit-{}.log", rand::random::<u64>()));
        let mut builder = crate::dns_conf::RuntimeConfig::builder();
        builder.audit.file = Some(file.clone());
        let cfg = builder.build().unwrap();
        let middleware = DnsAuditMiddleware::new(&cfg);
        let query = Query::query("single.example.".parse().unwrap(), RecordType::A);
        let response =
            DnsResponse::from_rdata(query.clone(), RData::A("192.0.2.4".parse().unwrap()));
        middleware
            .audit_sender
            .send(DnsAuditRecord {
                id: 1,
                client: "127.0.0.1".into(),
                query,
                result: Ok(response),
                speed: Duration::ZERO,
                elapsed: Duration::ZERO,
                date: Local::now(),
                lookup_source: LookupFrom::Static,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if std::fs::read_to_string(&file).is_ok_and(|s| s.contains("single.example.")) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn test_dns_audit_to_string() {
        let now = "2022-11-11 20:18:11.099966887 +08:00".parse().unwrap();
        let query = Query::query(Name::from_str("www.example.com").unwrap(), RecordType::A);
        let result = Ok(DnsResponse::from_rdata(
            query.to_owned(),
            RData::A("93.184.216.34".parse().unwrap()),
        ));

        let audit = DnsAuditRecord {
            id: 11,
            date: now,
            client: "127.0.0.1".to_string(),
            query,
            result,
            elapsed: Duration::from_millis(10),
            speed: Duration::from_millis(11),
            lookup_source: LookupFrom::Server("default".to_string()),
        };

        assert_eq!(
            audit.to_string(),
            format!(
                "[{}] 127.0.0.1 query www.example.com, type: A, elapsed: 10ms, speed: 11ms, result 93.184.216.34 86400 A",
                now.format("%Y-%m-%d %H:%M:%S,%3f")
            )
        );
    }

    #[test]
    fn test_dns_audit_to_string_without_date() {
        let now = "2022-11-11 20:18:11.099966887 +08:00".parse().unwrap();

        let query = Query::query(Name::from_str("www.example.com").unwrap(), RecordType::A);
        let result = Ok(DnsResponse::from_rdata(
            query.to_owned(),
            RData::A("93.184.216.34".parse().unwrap()),
        ));

        let audit = DnsAuditRecord {
            id: 11,
            date: now,
            client: "127.0.0.1".to_string(),
            query,
            result,
            elapsed: Duration::from_millis(10),
            speed: Duration::from_millis(11),
            lookup_source: LookupFrom::Server("default".to_string()),
        };

        assert_eq!(
            audit.to_string_without_date(),
            "127.0.0.1 query www.example.com, type: A, elapsed: 10ms, speed: 11ms, result 93.184.216.34 86400 A"
        );
    }

    #[test]
    fn test_record_audit_to_file() {
        let query = Query::query(Name::from_str("www.example.com").unwrap(), RecordType::A);

        let result = Ok(DnsResponse::from_rdata(
            query.to_owned(),
            RData::A("93.184.216.34".parse().unwrap()),
        ));

        let now = "2022-11-11 20:18:11.099966887 +08:00".parse().unwrap();

        let audit = DnsAuditRecord {
            id: 11,
            date: now,
            client: "127.0.0.1".to_string(),
            query,
            result,
            elapsed: Duration::from_millis(10),
            speed: Duration::from_millis(11),
            lookup_source: LookupFrom::Server("default".to_string()),
        };

        let file = format!("./logs/test-{}-audit.log", Local::now().timestamp_millis());
        let file = Path::new(file.as_str());

        record_audit_to_file(
            &mut MappedFile::open(file, 102400, None, Default::default()),
            &[audit],
        )
        .unwrap();

        assert!(file.exists());

        let mut s = String::new();

        std::fs::File::open(file)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();

        assert_eq!(
            s,
            format!(
                "[{}] 127.0.0.1 query www.example.com, type: A, elapsed: 10ms, speed: 11ms, result 93.184.216.34 86400 A\n",
                now.format("%Y-%m-%d %H:%M:%S,%3f")
            )
        );

        std::fs::remove_file(file).unwrap();

        assert!(!file.exists());
    }

    #[test]
    fn test_record_audit_to_csv_file() {
        let query = Query::query(Name::from_str("www.example.com").unwrap(), RecordType::A);

        let result = Ok(DnsResponse::from_rdata(
            query.to_owned(),
            RData::A("93.184.216.34".parse().unwrap()),
        ));

        let audit1 = DnsAuditRecord {
            id: 11,
            date: "2022-11-11 20:18:11.099966887 +08:00".parse().unwrap(),
            client: "127.0.0.1".to_string(),
            query: query.clone(),
            result: result.clone(),
            elapsed: Duration::from_millis(10),
            speed: Duration::from_millis(11),
            lookup_source: LookupFrom::Server("default1".to_string()),
        };

        let audit2 = DnsAuditRecord {
            id: 12,
            date: "2022-11-11 20:18:11.099966887 +08:00".parse().unwrap(),
            client: "127.0.0.1".to_string(),
            query,
            result,
            elapsed: Duration::from_millis(10),
            speed: Duration::from_millis(11),
            lookup_source: LookupFrom::Server("default2".to_string()),
        };

        let file = format!("./logs/test-{}-audit.csv", Local::now().timestamp_millis());
        let file = Path::new(file.as_str());

        record_audit_to_file(
            &mut MappedFile::open(file, 102400, None, Default::default()),
            &[audit1],
        )
        .unwrap();

        assert!(file.exists());

        let mut s = String::new();

        std::fs::File::open(file)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();

        assert_eq!(
            s,
            "id,timestamp,client,name,type,elapsed,speed,state,result,lookup_source\n11,1668169091,127.0.0.1,www.example.com,A,10ms,11ms,success,93.184.216.34 86400 A,Server: default1\n"
        );

        record_audit_to_file(
            &mut MappedFile::open(file, 102400, None, Default::default()),
            &[audit2],
        )
        .unwrap();

        let mut s = String::new();

        std::fs::File::open(file)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();

        assert_eq!(
            s,
            "id,timestamp,client,name,type,elapsed,speed,state,result,lookup_source\n11,1668169091,127.0.0.1,www.example.com,A,10ms,11ms,success,93.184.216.34 86400 A,Server: default1\n12,1668169091,127.0.0.1,www.example.com,A,10ms,11ms,success,93.184.216.34 86400 A,Server: default2\n"
        );

        std::fs::remove_file(file).unwrap();

        assert!(!file.exists());
    }
}
