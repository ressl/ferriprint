// SPDX-License-Identifier: AGPL-3.0-only
// Private QA helper for Parent review. Not staged or compiled yet.
// Fixed literal loopback listener. No outbound connection, job storage or document API.
// Only CUPS-Get-Printers and Get-Printer-Attributes are supported.
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use ferriprint::wire::*;
use futures_util::TryStreamExt;
use ipp::prelude::*;
use std::{
    os::unix::fs::OpenOptionsExt,
    sync::{atomic::{AtomicU64, Ordering}, Arc},
};
use tokio_util::io::StreamReader;

struct Metadata {
    ppd: Bytes,
    inventories: AtomicU64,
    attributes: AtomicU64,
    refused_operations: AtomicU64,
}

async fn real_ppd(State(metadata): State<Arc<Metadata>>) -> impl IntoResponse {
    ([("content-type", "application/vnd.cups-ppd")], metadata.ppd.clone())
}

async fn audit(State(metadata): State<Arc<Metadata>>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "inventories": metadata.inventories.load(Ordering::Relaxed),
        "attributes": metadata.attributes.load(Ordering::Relaxed),
        "refused_operations": metadata.refused_operations.load(Ordering::Relaxed),
        "jobs": [], "document_bytes_read": 0,
        "outbound_connection_code_exists": false
    }))
}

async fn receive(State(metadata): State<Arc<Metadata>>, request: Request) -> axum::response::Response {
    let mut stream = StreamReader::new(request.into_body().into_data_stream().map_err(std::io::Error::other));
    let request = match read_metadata(&mut stream, 256 * 1024).await {
        Ok(request) => request,
        Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
    };
    let operation = request.header().operation_or_status;
    let allowed = operation == 0x4002 || operation == 0x000b;
    let status = if allowed { StatusCode::SuccessfulOk } else { StatusCode::ServerErrorOperationNotSupported };
    let mut response = IppRequestResponse::new_response(IppVersion::v2_0(), status, request.header().request_id).unwrap();
    let group = DelimiterTag::PrinterAttributes;
    if operation == 0x4002 {
        metadata.inventories.fetch_add(1, Ordering::Relaxed);
        for (key, value) in [
            ("printer-name", name("receiver").unwrap()),
            ("printer-uri-supported", uri("ipp://127.0.0.1:18631/printers/receiver").unwrap()),
            ("printer-info", text("NONPRINTING published native.ppd metadata fixture").unwrap()),
            ("printer-make-and-model", text("Numeric Media Demonstrator (metadata fixture; no physical device)").unwrap()),
            ("printer-state", IppValue::Enum(3)),
            ("printer-is-accepting-jobs", IppValue::Boolean(true)),
        ] {
            add(&mut response, group, key, value).unwrap();
        }
    } else if operation == 0x000b {
        metadata.attributes.fetch_add(1, Ordering::Relaxed);
        for (key, value) in [
            ("printer-state", IppValue::Enum(3)),
            ("printer-is-accepting-jobs", IppValue::Boolean(true)),
            ("printer-state-reasons", keyword("none").unwrap()),
            ("document-format-supported", strings(&[
                "application/pdf".into(), "image/jpeg".into(), "image/png".into(),
                "image/pwg-raster".into(), "image/urf".into(),
            ], mime).unwrap()),
            ("urf-supported", strings(&[
                "V1.4".into(), "CP1".into(), "W8".into(), "OFU0".into(),
                "SRGB24".into(), "DM1".into(), "FN3".into(),
                "PQ3-4-5".into(), "RS300".into(),
            ], keyword).unwrap()),
            ("pwg-raster-document-type-supported", strings(&["sgray_8".into(), "srgb_8".into()], keyword).unwrap()),
        ] {
            add(&mut response, group, key, value).unwrap();
        }
    } else {
        metadata.refused_operations.fetch_add(1, Ordering::Relaxed);
        add(&mut response, DelimiterTag::OperationAttributes, "status-message",
            text("Metadata-only fixture: all other operations are forbidden").unwrap()).unwrap();
    }
    // Metadata parsing ends before document bytes. Drop the unread body.
    ([("content-type", "application/ipp")], Body::from(response.to_bytes())).into_response()
}

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).is_some_and(|v| v == "--observe" || v == "--observe-control" || v == "--observe-quiesce" || v == "--observe-ledger-snapshot") {
        // Never let a trace/parser panic or error print an input buffer.
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(observe::dispatch);
        std::process::exit(if matches!(result, Ok(Ok(()))) { 0 } else { 1 });
    }
    tokio::runtime::Runtime::new()?.block_on(receiver_main())
}

async fn receiver_main() -> anyhow::Result<()> {
    use std::io::Read;
    let mut args = std::env::args_os().skip(1);
    let ppd_path = args.next().ok_or_else(|| anyhow::anyhow!("one published public native.ppd fixture path required"))?;
    anyhow::ensure!(args.next().is_none(), "exactly one PPD path required");
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(ppd_path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "PPD must be a regular file");
    let mut ppd = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut ppd)?;
    anyhow::ensure!(ppd.len() <= 1024 * 1024 && ppd.starts_with(b"*PPD-Adobe:"), "invalid or oversized published PPD fixture");
    let metadata = Arc::new(Metadata {
        ppd: Bytes::from(ppd), inventories: AtomicU64::new(0),
        attributes: AtomicU64::new(0), refused_operations: AtomicU64::new(0),
    });
    let router = Router::new()
        .route("/", post(receive))
        .route("/printers/receiver", post(receive))
        .route("/printers/receiver.ppd", get(real_ppd))
        .route("/audit", get(audit))
        .with_state(metadata);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:18631").await?;
    println!("METADATA_ONLY_NONPRINTING_READY listen=127.0.0.1:18631");
    axum::serve(listener, router).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    }).await?;
    Ok(())
}

// Read-only, disposable-guest observation. This mode creates no listener and
// never sends, rewrites or exports HTTP, authentication or document buffers.
mod observe {
// QA-only ABI from the explicit derivative SQLite patch, not an upstream API.
const WAL_RETRY_REASON_IDS:[&str;15]=["INDEX_SHMMAP_BUSY", "INDEX_RECOVER_LOCK_RETRY", "READLOCK0_HEADER_CHANGED", "READMARK_NONE_BUSY", "READMARK_SHARED_LOCK_BUSY", "READMARK_OR_HEADER_CHANGED", "UNRELIABLE_READLOCK0_BUSY", "UNRELIABLE_NOW_RELIABLE", "UNRELIABLE_SHORT_WAL_NONEMPTY_INDEX", "UNRELIABLE_SALT_CHANGED", "UNRELIABLE_COMPLETE_TX_APPENDED", "PROTOCOL_LIMIT_REACHED", "DMS_GETLK_REPORTED_WRLCK", "DMS_EXCLUSIVE_ADVISORY_REQUEST_FAILED", "DMS_SHARED_ADVISORY_REQUEST_FAILED"];
unsafe extern "C" {
    fn ferriprint_qa_sqlite_wal_retry_counts_v2(out:*mut u32,count:libc::c_int,metadata:*mut libc::c_int,metadata_count:libc::c_int)->libc::c_int;
}
fn ledger_apply_counter_projection(value:&mut Value,read_count:i32,counts:&[u32;15],first:&[libc::c_int;3]) {
    value["wal_retry_counter_read_count"]=json!(read_count);
    if read_count==15 {
        value["wal_retry_reason_counts"]=Value::Object(WAL_RETRY_REASON_IDS.iter().zip(counts.iter()).map(|(id,count)|((*id).to_owned(),json!(count))).collect());
        // The recorded PID is the output-only field read back after the existing
        // F_GETLK. The caller supplied -2 as a known sentinel, so equality with
        // that sentinel means the syscall left the field untouched: it is the
        // caller's own input, not a reported holder. Only a differing value can
        // be a kernel-written result, and even then it stays unverified.
        let sentinel:libc::c_int=-2;
        let pid=if counts[12]>0 {Some(first[2])}else{None};
        value["dms_first_lock_returns"]=json!({
            "exclusive_errno":if counts[13]>0 {Some(first[0])}else{None},
            "shared_errno":if counts[14]>0 {Some(first[1])}else{None},
            "getlk_output_pid":pid,
            "getlk_output_pid_label":match pid {
                Some(value) if value==sentinel=>"UNCHANGED_INPUT_SENTINEL_NOT_A_HOLDER",
                Some(_)=>"DIFFERS_FROM_INPUT_SENTINEL_UNVERIFIED",
                None=>"NO_GUARD_BRANCH_OBSERVED"},
            "getlk_branch_count":counts[12],
            "getlk_branch_note":"The counted branch means the post-call l_type still equals the caller-supplied F_WRLCK type; it is not independent evidence of a competing holder."});
    }else{
        value["wal_retry_reason_counts"]=Value::Null;
        value["dms_first_lock_returns"]=Value::Null;
    }
}

fn ledger_child_failure_fields(id:&'static str,spawn_errno:Option<i32>,output:Option<&std::process::Output>,mut parsed:Option<Value>)->Result<Value> {
    if !matches!(id,"LEDGER_SERVICE_SPAWN"|"LEDGER_SERVICE_SNAPSHOT_FAILED"|"LEDGER_SERVICE_JSON"|"LEDGER_SERVICE_JSON_SHAPE"|"LEDGER_SERVICE_JSON_TYPE"|"LEDGER_SERVICE_IDENTITY"){return Err("LEDGER_WRAPPER_FAILURE_ID");}
    let Some(child)=output else{return Ok(json!({"wrapper_failure_id":id,"spawn_succeeded":false,
        "spawn_errno":spawn_errno,"child_success":null,"exit_code":null,"signal":null,"stdout_byte_count":null,
        "stdout_empty":null,"stdout_limit_exceeded":null,"stdout_JSON_valid":null,
        "child_failure_projection_valid":false,"child_failure":null}));};
    let within_limit=child.stdout.len()<=LEDGER_STDOUT_LIMIT;
    let valid_failure=within_limit&&parsed.as_ref().and_then(ledger_validated_failure).is_some();
    let mut diagnostic=json!({"wrapper_failure_id":id,"spawn_succeeded":true,"spawn_errno":null,
        "child_success":child.status.success(),"exit_code":child.status.code(),"signal":child.status.signal(),
        "stdout_byte_count":child.stdout.len(),"stdout_empty":child.stdout.is_empty(),"stdout_limit_exceeded":!within_limit,
        "stdout_JSON_valid":if within_limit{Some(parsed.is_some())}else{None},
        "child_failure_projection_valid":valid_failure,"child_failure":null});
    if valid_failure{diagnostic["child_failure"]=parsed.take().ok_or("LEDGER_FAILURE_PROJECTION_MISSING")?;}
    Ok(diagnostic)
}
const LEDGER_STDOUT_LIMIT:usize=4096;
#[derive(Default)]
struct LedgerFailureContext {
    stage:&'static str,
    uid:Option<u32>,euid:Option<u32>,gid:Option<u32>,egid:Option<u32>,
    getgroups_count:Option<i32>,getgroups_errno:Option<i32>,
    group_values_result:Option<i32>,group_values_errno:Option<i32>,
    group_ids:[libc::gid_t;32],group_ids_len:usize,group_ids_complete:bool,group_ids_truncated:bool,
    db_metadata_observed:bool,db_file_type_bits:Option<u32>,db_is_file:Option<bool>,db_is_symlink:Option<bool>,
    db_uid:Option<u32>,db_gid:Option<u32>,db_mode:Option<u32>,db_metadata_errno:Option<i32>,
    sqlite_error_code:Option<i32>,sqlite_extended_code:Option<i32>,
}
fn ledger_stage_known(stage:&str)->bool {
    matches!(stage,"CREDENTIALS"|"DB_METADATA"|"DB_IDENTITY"|"DB_OPEN"|"DB_TRANSACTION"|"JOB_COUNT"|
        "ACTIVE_JOB_COUNT"|"MAX_JOB_ID"|"SQLITE_SEQUENCE"|"TIME_ORIGIN"|"ZERO_JOB_GUARD"|"SERIALIZE"|"STDOUT"|"INTERNAL")
}
fn ledger_failure_allowed(id:&str,stage:&str)->bool {
    match id {
        "LEDGER_SERVICE_CREDENTIALS"=>stage=="CREDENTIALS",
        "RESTART_LEDGER_METADATA"=>stage=="DB_METADATA",
        "RESTART_LEDGER_IDENTITY"=>stage=="DB_IDENTITY",
        "RESTART_LEDGER_READONLY_OPEN"=>stage=="DB_OPEN",
        "RESTART_LEDGER_READONLY_SNAPSHOT"=>stage=="DB_TRANSACTION",
        "RESTART_LEDGER_COUNT"=>matches!(stage,"JOB_COUNT"|"ACTIVE_JOB_COUNT"),
        "RESTART_LEDGER_IDS"=>stage=="MAX_JOB_ID",
        "RESTART_LEDGER_SEQUENCE"=>stage=="SQLITE_SEQUENCE",
        "RESTART_LEDGER_TIME_ORIGIN"=>stage=="TIME_ORIGIN",
        "RESTART_LEDGER_NOT_ZERO"=>stage=="ZERO_JOB_GUARD",
        "LEDGER_SERVICE_JSON"|"LEDGER_SERVICE_JSON_BOUND"=>stage=="SERIALIZE",
        "LEDGER_SERVICE_STDOUT"=>stage=="STDOUT",
        "LEDGER_SERVICE_PANIC"=>ledger_stage_known(stage),
        _=>false,
    }
}
impl LedgerFailureContext {
    fn credentials(&mut self) {
        self.stage="CREDENTIALS";
        self.uid=Some(unsafe {libc::getuid()});self.euid=Some(unsafe {libc::geteuid()});
        self.gid=Some(unsafe {libc::getgid()});self.egid=Some(unsafe {libc::getegid()});
        let count=unsafe {libc::getgroups(0,std::ptr::null_mut())};
        self.getgroups_errno=if count<0 {std::io::Error::last_os_error().raw_os_error()}else{None};
        self.getgroups_count=Some(count);
        // Keep literal count0 rejection unchanged. getgroups may include EGID:
        // values are raw observations, NOT proof that every returned GID is extra.
        if count>0 && count<=32 {
            let result=unsafe {libc::getgroups(32,self.group_ids.as_mut_ptr())};
            self.group_values_errno=if result<0 {std::io::Error::last_os_error().raw_os_error()}else{None};
            self.group_values_result=Some(result);
            if (0..=32).contains(&result){self.group_ids_len=result as usize;self.group_ids_complete=result==count;}
            else if result>32 {self.group_ids_truncated=true;}
        }else if count==0{self.group_ids_complete=true;}
        else if count>32{self.group_ids_truncated=true;}
    }
    fn sqlite_failure(&mut self,error:rusqlite::Error,id:&'static str)->&'static str {
        if let rusqlite::Error::SqliteFailure(code,_)=error {
            self.sqlite_extended_code=Some(code.extended_code);
            self.sqlite_error_code=Some(code.extended_code&0xff);
        }
        id
    }
    fn projection(&self,id:&'static str)->Result<Value> {
        if !ledger_failure_allowed(id,self.stage)||self.group_ids_len>32{return Err("LEDGER_FAILURE_PROJECTION_ID");}
        Ok(json!({"schema_version":3,"stage_id":self.stage,"error_id":id,
            "uid":self.uid,"euid":self.euid,"gid":self.gid,"egid":self.egid,
            "getgroups_count":self.getgroups_count,"getgroups_errno":self.getgroups_errno,
            "group_values_result":self.group_values_result,"group_values_errno":self.group_values_errno,
            "group_ids":&self.group_ids[..self.group_ids_len],"group_ids_complete":self.group_ids_complete,"group_ids_truncated":self.group_ids_truncated,
            "db_metadata_observed":self.db_metadata_observed,"db_file_type_bits":self.db_file_type_bits,
            "db_is_file":self.db_is_file,"db_is_symlink":self.db_is_symlink,"db_uid":self.db_uid,"db_gid":self.db_gid,
            "db_mode":self.db_mode,"db_metadata_errno":self.db_metadata_errno,
            "sqlite_error_code":self.sqlite_error_code,"sqlite_extended_code":self.sqlite_extended_code,
            "wal_retry_counter_schema":2,"wal_retry_counter_read_count":null,"wal_retry_reason_counts":null,"dms_first_lock_returns":null}))
    }
}
fn ledger_validated_failure(value:&Value)->Option<&Value> {
    let object=value.as_object()?;
    let keys=["schema_version","stage_id","error_id","uid","euid","gid","egid","getgroups_count","getgroups_errno",
        "group_values_result","group_values_errno","group_ids","group_ids_complete","group_ids_truncated","db_metadata_observed",
        "db_file_type_bits","db_is_file","db_is_symlink","db_uid","db_gid","db_mode","db_metadata_errno","sqlite_error_code","sqlite_extended_code","wal_retry_counter_schema","wal_retry_counter_read_count","wal_retry_reason_counts","dms_first_lock_returns"];
    if object.len()!=keys.len()||!keys.iter().all(|key|object.contains_key(*key))||value["schema_version"]!=3{return None;}
    if !ledger_failure_allowed(value["error_id"].as_str()?,value["stage_id"].as_str()?){return None;}
    for key in ["uid","euid","gid","egid","db_uid","db_gid"] {
        if !value[key].is_null()&&!value[key].as_u64().is_some_and(|v|v<=u32::MAX as u64){return None;}
    }
    for key in ["getgroups_count","group_values_result"] {
        if !value[key].is_null()&&!value[key].as_i64().is_some_and(|v|(-1..=i32::MAX as i64).contains(&v)){return None;}
    }
    for key in ["getgroups_errno","group_values_errno","db_metadata_errno"] {
        if !value[key].is_null()&&!value[key].as_i64().is_some_and(|v|(1..=i32::MAX as i64).contains(&v)){return None;}
    }
    for key in ["group_ids_complete","group_ids_truncated","db_metadata_observed"] {value[key].as_bool()?;}
    for key in ["db_is_file","db_is_symlink"] {if !value[key].is_null(){value[key].as_bool()?;}}
    for (key,max) in [("db_file_type_bits",0o170000u64),("db_mode",0o7777u64)] {
        if !value[key].is_null()&&!value[key].as_u64().is_some_and(|v|v<=max){return None;}
    }
    let groups=value["group_ids"].as_array()?;
    if groups.len()>32||groups.iter().any(|v|!v.as_u64().is_some_and(|v|v<=u32::MAX as u64)){return None;}
    match (value["sqlite_error_code"].as_i64(),value["sqlite_extended_code"].as_i64()) {
        (Some(code),Some(extended)) if (0..=255).contains(&code)&&(0..=i32::MAX as i64).contains(&extended)&&code==(extended&0xff)=>{},
        (None,None) if value["sqlite_error_code"].is_null()&&value["sqlite_extended_code"].is_null()=>{},
        _=>return None,
    }
    if value["wal_retry_counter_schema"]!=2{return None;}
    match value["wal_retry_counter_read_count"].as_i64() {
        Some(15)=>{
            let counts=value["wal_retry_reason_counts"].as_object()?;
            if counts.len()!=WAL_RETRY_REASON_IDS.len()||!WAL_RETRY_REASON_IDS.iter().all(|id|counts.get(*id).is_some_and(|v|v.as_u64().is_some_and(|n|n<=u32::MAX as u64))){return None;}
            let first=value["dms_first_lock_returns"].as_object()?;
            let first_keys=["exclusive_errno","shared_errno","getlk_output_pid","getlk_output_pid_label",
                "getlk_branch_count","getlk_branch_note"];
            if first.len()!=first_keys.len()||!first_keys.iter().all(|key|first.contains_key(*key)){return None;}
            for (field,reason) in [("exclusive_errno",13usize),("shared_errno",14usize)] {
                let seen=counts.get(WAL_RETRY_REASON_IDS[reason])?.as_u64()?>0;
                if seen {
                    if !first[field].as_i64().is_some_and(|v|(0..=i32::MAX as i64).contains(&v)){return None;}
                }else if !first[field].is_null(){return None;}
            }
            let guard_branches=counts.get(WAL_RETRY_REASON_IDS[12])?.as_u64()?;
            if first["getlk_branch_count"].as_u64()!=Some(guard_branches){return None;}
            if guard_branches>0 {
                let pid=first["getlk_output_pid"].as_i64().filter(|v|(i32::MIN as i64..=i32::MAX as i64).contains(v))?;
                let label=first["getlk_output_pid_label"].as_str()?;
                let expected=if pid==-2 {"UNCHANGED_INPUT_SENTINEL_NOT_A_HOLDER"}else{"DIFFERS_FROM_INPUT_SENTINEL_UNVERIFIED"};
                if label!=expected{return None;}
            }else if !first["getlk_output_pid"].is_null()
                ||first["getlk_output_pid_label"].as_str()!=Some("NO_GUARD_BRANCH_OBSERVED"){return None;}
            if first["getlk_branch_note"].as_str()!=Some("The counted branch means the post-call l_type still equals the caller-supplied F_WRLCK type; it is not independent evidence of a competing holder."){return None;}
        },
        Some(0)=>{if !value["wal_retry_reason_counts"].is_null()||!value["dms_first_lock_returns"].is_null(){return None;}},
        None if value["wal_retry_counter_read_count"].is_null()=>{if !value["wal_retry_reason_counts"].is_null()||!value["dms_first_lock_returns"].is_null(){return None;}},
        _=>return None,
    }
    Some(value)
}
fn ledger_emit_failure(context:&LedgerFailureContext,id:&'static str)->Result<()> {
    let mut value=context.projection(id)?;
    let mut counts=[0u32;15];let mut first=[0 as libc::c_int;3];
    let read_count=unsafe {ferriprint_qa_sqlite_wal_retry_counts_v2(counts.as_mut_ptr(),15,first.as_mut_ptr(),3)};
    ledger_apply_counter_projection(&mut value,read_count,&counts,&first);
    let bytes=serde_json::to_vec(&value).map_err(|_|"LEDGER_SERVICE_JSON")?;
    if bytes.len()>LEDGER_STDOUT_LIMIT{return Err("LEDGER_SERVICE_JSON_BOUND");}
    let mut stdout=std::io::stdout().lock();
    stdout.write_all(&bytes).and_then(|_|stdout.flush()).map_err(|_|"LEDGER_SERVICE_STDOUT")
}
fn ledger_record_child_failure(work:&Path,value:Value)->Result<()> {
    let mut proof=load_restart(work)?;
    if proof.get("ledger_snapshot_child_failure").is_none(){proof["ledger_snapshot_child_failure"]=value;}
    proof["preservation_required"]=json!(true);save_restart(work,&proof)
}
fn ledger_preserve_child_failure(work:&Path,value:&mut Value)->Result<()> {
    let saved=load_restart(work)?;
    if let Some(diagnostic)=saved.get("ledger_snapshot_child_failure"){value["ledger_snapshot_child_failure"]=diagnostic.clone();}
    Ok(())
}

fn kernel_lock_argument(text:&str,command:&str,returned:i64)->Result<()> {
    if text.len()>512{return Err("KERNEL_FCNTL_LOCK_BOUND");}
    if text=="NULL" || text.starts_with("0x") {
        kernel_pointer(text)?;
        if returned>=0{return Err("KERNEL_FCNTL_LOCK_OPAQUE");}return Ok(());
    }
    let get=matches!(command,"F_GETLK"|"F_GETLK64"|"F_OFD_GETLK");
    let inner=text.strip_prefix('{').and_then(|v|v.strip_suffix('}')).ok_or("KERNEL_FCNTL_LOCK_GRAMMAR")?;
    let fields=kernel_fields(inner,false)?;fields.arity(if get {5}else{4})?;
    if !matches!(fields.get(0)?,"l_type=F_RDLCK"|"l_type=F_WRLCK"|"l_type=F_UNLCK") ||
        !matches!(fields.get(1)?,"l_whence=SEEK_SET"|"l_whence=SEEK_CUR"|"l_whence=SEEK_END") {return Err("KERNEL_FCNTL_LOCK_GRAMMAR");}
    for (at,prefix) in [(2,"l_start="),(3,"l_len=")] {
        let value=fields.get(at)?.strip_prefix(prefix).ok_or("KERNEL_FCNTL_LOCK_GRAMMAR")?;
        if value.starts_with('-'){value.parse::<i64>().map_err(|_|"KERNEL_FCNTL_LOCK_NUMBER")?;}
        else{i64::try_from(kernel_u64(value)?).map_err(|_|"KERNEL_FCNTL_LOCK_NUMBER")?;}
    }
    if get{kernel_i32(fields.get(4)?.strip_prefix("l_pid=").ok_or("KERNEL_FCNTL_LOCK_GRAMMAR")?)?;}
    Ok(())
}
// Finite syscall-origin FD model. Result, Value, json! and BTreeMap come from
// the enclosing module. No decoded trace text is retained or projected here.
const KERNEL_FD_SLOT_LIMIT: usize = 4096;
const KERNEL_FD_PENDING_LIMIT: usize = 128;
const KERNEL_FD_TEXT_LIMIT: usize = 1024 * 1024;

struct KernelFdEvent {
    io_target: Option<u64>,
    retired_targets: Vec<u64>,
    verify_exec: bool,
}
impl KernelFdEvent {
    fn empty() -> Self {
        Self { io_target: None, retired_targets: Vec::new(), verify_exec: false }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelCall {
    Socket, Socketpair, Bind, Listen, Connect, Accept, Accept4,
    Dup, Dup2, Dup3, Fcntl, Close, CloseRange,
    Open, Openat, Openat2, Creat, Pipe, Pipe2, Eventfd, Eventfd2,
    EpollCreate, EpollCreate1, Signalfd, Signalfd4, TimerfdCreate,
    Clone, Clone3, Fork, Vfork, Unshare, Execve, Execveat,
    Read, Readv, Recvfrom, Recvmsg, Write, Writev, Sendto, Sendmsg,
}
impl KernelCall {
    fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "socket" => Self::Socket, "socketpair" => Self::Socketpair,
            "bind" => Self::Bind, "listen" => Self::Listen, "connect" => Self::Connect,
            "accept" => Self::Accept, "accept4" => Self::Accept4,
            "dup" => Self::Dup, "dup2" => Self::Dup2, "dup3" => Self::Dup3,
            "fcntl" => Self::Fcntl, "close" => Self::Close, "close_range" => Self::CloseRange,
            "open" => Self::Open, "openat" => Self::Openat, "openat2" => Self::Openat2,
            "creat" => Self::Creat, "pipe" => Self::Pipe, "pipe2" => Self::Pipe2,
            "eventfd" => Self::Eventfd, "eventfd2" => Self::Eventfd2,
            "epoll_create" => Self::EpollCreate, "epoll_create1" => Self::EpollCreate1,
            "signalfd" => Self::Signalfd, "signalfd4" => Self::Signalfd4,
            "timerfd_create" => Self::TimerfdCreate,
            "clone" => Self::Clone, "clone3" => Self::Clone3,
            "fork" => Self::Fork, "vfork" => Self::Vfork, "unshare" => Self::Unshare,
            "execve" => Self::Execve, "execveat" => Self::Execveat,
            "read" => Self::Read, "readv" => Self::Readv,
            "recvfrom" => Self::Recvfrom, "recvmsg" => Self::Recvmsg,
            "write" => Self::Write, "writev" => Self::Writev,
            "sendto" => Self::Sendto, "sendmsg" => Self::Sendmsg,
            _ => return Err("KERNEL_SYSCALL_UNSUPPORTED"),
        })
    }
    fn id(self) -> u8 { self as u8 + 1 }
    fn io(self) -> bool {
        matches!(self, Self::Read | Self::Readv | Self::Recvfrom | Self::Recvmsg |
            Self::Write | Self::Writev | Self::Sendto | Self::Sendmsg)
    }
    fn exec(self) -> bool { matches!(self, Self::Execve | Self::Execveat) }
    fn socket_operation(self) -> bool {
        matches!(self, Self::Socket | Self::Socketpair | Self::Bind | Self::Listen |
            Self::Connect | Self::Accept | Self::Accept4)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelFdKind { NonSocket, Socket(u64) }
#[derive(Clone, Copy)]
struct KernelFdSlot { epoch: u64, live: Option<KernelFdKind>, cloexec: bool }
#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelSocketClass { Tcp4, NonTarget, Unknown }
impl KernelSocketClass {
    fn id(self) -> u8 { match self { Self::Tcp4 => 1, Self::NonTarget => 2, Self::Unknown => 3 } }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelSocketState {
    Unconnected, BoundTarget, BoundOther, TargetListener, OtherListener,
    AcceptedTarget, BackendPending, BackendConnected, OtherConnected,
}
impl KernelSocketState {
    fn id(self) -> u8 { self as u8 + 1 }
}
#[derive(Clone, Copy)]
struct KernelSocketOrigin {
    class: KernelSocketClass,
    family_id: u8,
    type_id: u8,
    protocol_id: u8,
    state: KernelSocketState,
    version: u64,
    aliases: u32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct KernelFdInput {
    fd: i32,
    epoch: u64,
    socket_generation: u64,
    socket_version: u64,
}
impl KernelFdInput {
    const EMPTY: Self = Self { fd: -1, epoch: 0, socket_generation: 0, socket_version: 0 };
}
#[derive(Clone, Copy)]
struct KernelFdEntry {
    call: KernelCall,
    sequence: u64,
    inputs: [KernelFdInput; 2],
    input_count: usize,
    range: Option<(u32, u32)>,
    write_fd: Option<i32>,
    write_socket: u64,
    flags: u64,
    creates_fd: bool,
}
impl KernelFdEntry {
    fn blocks(&self, input: KernelFdInput) -> bool {
        self.call.exec() || self.write_fd == Some(input.fd) ||
            (self.write_socket != 0 && self.write_socket == input.socket_generation) ||
            self.range.is_some_and(|(first, last)| input.fd >= 0 &&
                (input.fd as u32) >= first && (input.fd as u32) <= last)
    }
}

struct KernelFdOrigins {
    // Tombstones stay bounded too: absence after a close cannot erase a reuse epoch.
    slots: BTreeMap<i32, KernelFdSlot>,
    sockets: BTreeMap<u64, KernelSocketOrigin>,
    pending: BTreeMap<u32, KernelFdEntry>,
    resumed: Option<KernelFdEntry>,
    sequence: u64,
    next_socket: u64,
    event_count: u64,
    initial_fd_count: usize,
    socket_birth_count: u64,
    target_accept_count: u64,
    retired_target_count: u64,
    shared_clone_count: u64,
    target_bound_generation: u64,
    target_listener_generation: u64,
    exec_started: bool,
    exec_succeeded: bool,
    awaiting_exec_confirmation: bool,
    exec_confirmed: bool,
    failed: bool,
}
impl KernelFdOrigins {
    fn new(initial_fds: &[i32]) -> Result<Self> {
        if initial_fds.len() > 64 { return Err("KERNEL_INITIAL_FD_LIMIT"); }
        let mut model = Self {
            slots: BTreeMap::new(), sockets: BTreeMap::new(), pending: BTreeMap::new(), resumed: None,
            sequence: 0, next_socket: 0, event_count: 0, initial_fd_count: initial_fds.len(),
            socket_birth_count: 0, target_accept_count: 0, retired_target_count: 0, shared_clone_count: 0,
            target_bound_generation: 0, target_listener_generation: 0,
            exec_started: false, exec_succeeded: false, awaiting_exec_confirmation: false,
            exec_confirmed: false, failed: false,
        };
        // The caller's controlled, inherited inventory proves these non-socket,
        // non-CLOEXEC descriptors; no pathname or annotation is an inventory source.
        for &fd in initial_fds {
            if fd < 0 || model.slots.contains_key(&fd) { return Err("KERNEL_INITIAL_FD_INVENTORY"); }
            let epoch = model.advance()?;
            model.slots.insert(fd, KernelFdSlot { epoch, live: Some(KernelFdKind::NonSocket), cloexec: false });
        }
        Ok(model)
    }
    fn advance(&mut self) -> Result<u64> {
        self.sequence = self.sequence.checked_add(1).ok_or("KERNEL_EPOCH_LIMIT")?;
        Ok(self.sequence)
    }
    fn usable(&self) -> Result<()> {
        if self.failed { return Err("KERNEL_FD_MODEL_FAILED"); }
        if self.awaiting_exec_confirmation { return Err("KERNEL_EXEC_CONFIRMATION_REQUIRED"); }
        Ok(())
    }
    fn snapshot(&self, fd: i32) -> Result<KernelFdInput> {
        let mut input = KernelFdInput { fd, ..KernelFdInput::EMPTY };
        if let Some(slot) = self.slots.get(&fd) {
            input.epoch = slot.epoch;
            if let Some(KernelFdKind::Socket(generation)) = slot.live {
                let socket = self.sockets.get(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
                input.socket_generation = generation;
                input.socket_version = socket.version;
            }
        }
        Ok(input)
    }
    fn capture(&self, entry: &mut KernelFdEntry, fd: i32) -> Result<()> {
        if fd < 0 { return Ok(()); }
        if entry.inputs[..entry.input_count].iter().any(|input| input.fd == fd) { return Ok(()); }
        if entry.input_count == entry.inputs.len() { return Err("KERNEL_ENTRY_INPUT_LIMIT"); }
        entry.inputs[entry.input_count] = self.snapshot(fd)?;
        entry.input_count += 1;
        Ok(())
    }
    fn socket_for(&self, fd: i32) -> Result<(u64, KernelSocketOrigin)> {
        let Some(KernelFdSlot { live: Some(KernelFdKind::Socket(generation)), .. }) = self.slots.get(&fd)
            else { return Err("KERNEL_SOCKET_ORIGIN_UNKNOWN"); };
        Ok((*generation, *self.sockets.get(generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?))
    }
    fn check_entry(&self, entry: &KernelFdEntry) -> Result<()> {
        for input in &entry.inputs[..entry.input_count] {
            if self.snapshot(input.fd)? != *input { return Err("KERNEL_FD_ENTRY_EPOCH_CHANGED"); }
        }
        if let Some((first, last)) = entry.range {
            for (&fd, slot) in &self.slots {
                if (fd as u32) < first || (fd as u32) > last { continue; }
                if slot.epoch > entry.sequence { return Err("KERNEL_RANGE_ENTRY_EPOCH_CHANGED"); }
                if let Some(KernelFdKind::Socket(generation)) = slot.live {
                    if self.sockets.get(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?.version > entry.sequence {
                        return Err("KERNEL_RANGE_ENTRY_EPOCH_CHANGED");
                    }
                }
            }
        }
        Ok(())
    }
    fn conflicts(&self, entry: &KernelFdEntry) -> Result<()> {
        for pending in self.pending.values() {
            for input in &entry.inputs[..entry.input_count] {
                if pending.blocks(*input) { return Err("KERNEL_FD_PENDING_OVERLAP"); }
            }
            for input in &pending.inputs[..pending.input_count] {
                if entry.blocks(*input) { return Err("KERNEL_FD_PENDING_OVERLAP"); }
            }
            if let (Some((a, b)), Some((c, d))) = (entry.range, pending.range) {
                if a <= d && c <= b { return Err("KERNEL_FD_PENDING_OVERLAP"); }
            }
            // A creator may already have allocated an as-yet-unprinted FD.
            // A range operation cannot safely omit that invisible generation.
            if (entry.range.is_some() && pending.creates_fd) ||
                (pending.range.is_some() && entry.creates_fd) {
                return Err("KERNEL_RANGE_PENDING_CREATOR");
            }
            if entry.call.exec() || pending.call.exec() { return Err("KERNEL_EXEC_PENDING_OVERLAP"); }
        }
        Ok(())
    }
    fn describe(&self, call: KernelCall, fields: &KernelFields<'_>, prefix: bool) -> Result<KernelFdEntry> {
        if call.socket_operation() && !self.exec_confirmed { return Err("KERNEL_SOCKET_BEFORE_VERIFIED_EXEC"); }
        let mut entry = KernelFdEntry {
            call, sequence: self.sequence, inputs: [KernelFdInput::EMPTY; 2], input_count: 0,
            range: None, write_fd: None, write_socket: 0, flags: 0,
            creates_fd: matches!(call, KernelCall::Socket | KernelCall::Socketpair |
                KernelCall::Accept | KernelCall::Accept4 | KernelCall::Dup |
                KernelCall::Open | KernelCall::Openat | KernelCall::Openat2 | KernelCall::Creat |
                KernelCall::Pipe | KernelCall::Pipe2 | KernelCall::Eventfd | KernelCall::Eventfd2 |
                KernelCall::EpollCreate | KernelCall::EpollCreate1 | KernelCall::TimerfdCreate),
        };
        if call.io() || matches!(call, KernelCall::Bind | KernelCall::Listen | KernelCall::Connect |
            KernelCall::Accept | KernelCall::Accept4 | KernelCall::Dup | KernelCall::Dup2 |
            KernelCall::Dup3 | KernelCall::Fcntl | KernelCall::Close | KernelCall::Signalfd | KernelCall::Signalfd4) {
            let fd = kernel_fd(fields.get(0)?)?;
            self.validate_fd(fd)?;
            self.capture(&mut entry, fd.fd)?;
            if matches!(call, KernelCall::Bind | KernelCall::Listen | KernelCall::Connect) {
                entry.write_socket = entry.inputs.first().map_or(0, |input| input.socket_generation);
                if entry.write_socket == 0 && fd.fd >= 0 { entry.write_fd = Some(fd.fd); }
            }
            if call == KernelCall::Close && fd.fd >= 0 { entry.write_fd = Some(fd.fd); }
            if matches!(call, KernelCall::Signalfd | KernelCall::Signalfd4) && fd.fd == -1 { entry.creates_fd = true; }
            if matches!(call, KernelCall::Dup2 | KernelCall::Dup3) {
                let target = kernel_fd(fields.get(1)?)?;
                self.validate_fd(target)?;
                self.capture(&mut entry, target.fd)?;
                if target.fd >= 0 && (call == KernelCall::Dup3 || target.fd != fd.fd) {
                    entry.write_fd = Some(target.fd);
                }
            }
            if call == KernelCall::Fcntl {
                match fields.get(1)? {
                    "F_DUPFD" | "F_DUPFD_CLOEXEC" => entry.creates_fd = true,
                    "F_GETFD" | "F_GETFL" | "F_SETFL" | "F_SETLK" | "F_SETLKW" | "F_GETLK" | "F_SETLK64" | "F_SETLKW64" | "F_GETLK64" | "F_OFD_SETLK" | "F_OFD_SETLKW" | "F_OFD_GETLK" => {},
                    "F_SETFD" => {
                        entry.flags = kernel_named_flags(fields.get(2)?, &[("FD_CLOEXEC", 1)])?;
                        if fd.fd >= 0 { entry.write_fd = Some(fd.fd); }
                    },
                    _ => return Err("KERNEL_FCNTL_UNSUPPORTED"),
                }
            }
        }
        match call {
            KernelCall::Openat | KernelCall::Openat2 => {
                let fd = kernel_dirfd(fields.get(0)?)?;
                self.validate_fd(fd)?;
                self.capture(&mut entry, fd.fd)?;
            },
            KernelCall::CloseRange => {
                let first = kernel_u32(fields.get(0)?)?;
                let last = kernel_u32(fields.get(1)?)?;
                if first > last { return Err("KERNEL_CLOSE_RANGE_ORDER"); }
                let flags = kernel_named_flags(fields.get(2)?, &[("CLOSE_RANGE_UNSHARE", 2), ("CLOSE_RANGE_CLOEXEC", 4)])?;
                if flags & 2 != 0 { return Err("KERNEL_CLOSE_RANGE_UNSHARE_UNSUPPORTED"); }
                entry.range = Some((first, last)); entry.flags = flags;
            },
            KernelCall::Clone | KernelCall::Clone3 => {
                if !self.exec_confirmed { return Err("KERNEL_CLONE_BEFORE_VERIFIED_EXEC"); }
                entry.flags = kernel_clone_flags(call, fields, prefix)?;
            },
            KernelCall::Fork | KernelCall::Vfork | KernelCall::Unshare => return Err("KERNEL_UNSHARED_TABLE_UNSUPPORTED"),
            KernelCall::Execve | KernelCall::Execveat => {
                let dirfd = kernel_raw_exec(call, fields)?;
                if let Some(fd) = dirfd { self.capture(&mut entry, fd)?; }
            },
            _ => {},
        }
        Ok(entry)
    }
    fn note_entry(&mut self, tid: u32, text: &str) -> Result<()> {
        let result = self.note_entry_inner(tid, text);
        if result.is_err() { self.failed = true; }
        result
    }
    fn note_entry_inner(&mut self, tid: u32, text: &str) -> Result<()> {
        self.usable()?;
        if tid == 0 || self.resumed.is_some() || self.pending.contains_key(&tid) { return Err("KERNEL_DOUBLE_PENDING"); }
        if self.pending.len() >= KERNEL_FD_PENDING_LIMIT || text.len() > KERNEL_FD_TEXT_LIMIT {
            return Err("KERNEL_PENDING_LIMIT");
        }
        let text = text.trim_end();
        let text = text.strip_suffix("<unfinished ...>").unwrap_or(text).trim_end();
        let (name, args) = text.split_once('(').ok_or("KERNEL_ENTRY_SYNTAX")?;
        let call = KernelCall::parse(name)?;
        let fields = kernel_fields(args, true)?;
        let entry = self.describe(call, &fields, true)?;
        self.conflicts(&entry)?;
        if call.exec() { self.begin_exec()?; }
        self.pending.insert(tid, entry);
        Ok(())
    }
    fn note_resume(&mut self, tid: u32) -> Result<()> {
        let result = self.note_resume_inner(tid);
        if result.is_err() { self.failed = true; }
        result
    }
    fn note_resume_inner(&mut self, tid: u32) -> Result<()> {
        self.usable()?;
        if self.resumed.is_some() { return Err("KERNEL_RESUME_NOT_CONSUMED"); }
        let entry = *self.pending.get(&tid).ok_or("KERNEL_UNMATCHED_RESUMED")?;
        self.check_entry(&entry)?;
        self.pending.remove(&tid);
        self.resumed = Some(entry);
        Ok(())
    }
    fn begin_exec(&mut self) -> Result<()> {
        if self.exec_started || self.socket_birth_count != 0 || !self.sockets.is_empty() {
            return Err("KERNEL_EXEC_NOT_SINGLE_STARTUP");
        }
        if !self.pending.is_empty() { return Err("KERNEL_EXEC_PENDING_OVERLAP"); }
        self.exec_started = true;
        Ok(())
    }
    fn confirm_published_exec(&mut self) -> Result<()> {
        let result = self.confirm_published_exec_inner();
        if result.is_err() { self.failed = true; }
        result
    }
    fn confirm_published_exec_inner(&mut self) -> Result<()> {
        if self.failed { return Err("KERNEL_FD_MODEL_FAILED"); }
        if !self.exec_succeeded || !self.awaiting_exec_confirmation || self.exec_confirmed ||
            self.socket_birth_count != 0 || !self.pending.is_empty() || self.resumed.is_some() {
            return Err("KERNEL_EXEC_CONFIRMATION_ORDER");
        }
        // The only successful exec is before every socket; CLOEXEC retirement
        // therefore cannot discard an HTTP target or need a retired-target output.
        let mut cursor = 0i32;
        let mut event = KernelFdEvent::empty();
        loop {
            let next = self.slots.range(cursor..).next().map(|(&fd, slot)| (fd, slot.live.is_some() && slot.cloexec));
            let Some((fd, close)) = next else { break; };
            if close { self.close_slot(fd, &mut event)?; }
            let Some(after) = fd.checked_add(1) else { break; }; cursor = after;
        }
        if !event.retired_targets.is_empty() { return Err("KERNEL_EXEC_TARGET_RETIREMENT"); }
        self.awaiting_exec_confirmation = false;
        self.exec_confirmed = true;
        Ok(())
    }
    fn event(&mut self, call: &str, args: &str, returned: i64, result_text: &str) -> Result<KernelFdEvent> {
        let result = self.event_inner(call, args, returned, result_text);
        if result.is_err() { self.failed = true; }
        result
    }
    fn event_inner(&mut self, name: &str, args: &str, returned: i64, result_text: &str) -> Result<KernelFdEvent> {
            self.usable()?;
            if args.len() > KERNEL_FD_TEXT_LIMIT || result_text.len() > KERNEL_FD_TEXT_LIMIT { return Err("KERNEL_EVENT_LIMIT"); }
            let call = KernelCall::parse(name)?;
            let fields = kernel_fields(args, false)?;
            let result = kernel_result(call, returned, result_text)?;
            let resumed = self.resumed.take();
            let entry = self.describe(call, &fields, false)?;
            if let Some(previous) = resumed {
                if previous.call != call || previous.input_count != entry.input_count ||
                    previous.inputs != entry.inputs || previous.range != entry.range || previous.flags != entry.flags {
                    return Err("KERNEL_RESUMED_INPUT_CONFLICT");
                }
                self.check_entry(&previous)?;
            }
            self.conflicts(&entry)?;
            self.event_count = self.event_count.checked_add(1).ok_or("KERNEL_EVENT_COUNT_LIMIT")?;
            let mut event = KernelFdEvent::empty();
            if call.exec() {
                if resumed.is_none() { self.begin_exec()?; }
                if returned != 0 || result.restarted || result.errno != KernelErrno::None || !self.pending.is_empty() {
                    return Err("KERNEL_STARTUP_EXEC_FAILED");
                }
                self.exec_succeeded = true;
                self.awaiting_exec_confirmation = true;
                event.verify_exec = true;
                return Ok(event);
            }
            if call.io() {
                fields.arity(match call { KernelCall::Recvfrom | KernelCall::Sendto => 6, _ => 3 })?;
                let fd = kernel_fd(fields.get(0)?)?;
                if returned > 0 { event.io_target = self.target_role(fd.fd)?; }
                return Ok(event);
            }
            match call {
                KernelCall::Socket | KernelCall::Socketpair => {
                    fields.arity(if call == KernelCall::Socket { 3 } else { 4 })?;
                    let (mut class, cloexec, identity) = kernel_socket_spec(fields.get(0)?, fields.get(1)?, fields.get(2)?)?;
                    if call == KernelCall::Socketpair && class == KernelSocketClass::Tcp4 { class = KernelSocketClass::Unknown; }
                    if returned >= 0 {
                        if call == KernelCall::Socket {
                            let fd = kernel_return_fd(returned)?;
                            self.birth_socket(fd, class, identity, KernelSocketState::Unconnected, cloexec, resumed.as_ref())?;
                            self.validate_annotation(fd, result.annotation)?;
                        } else {
                            if returned != 0 { return Err("KERNEL_SOCKETPAIR_RESULT"); }
                            let pair = kernel_fd_pair(fields.get(3)?)?;
                            for fd in pair {
                                self.birth_socket(fd.fd, class, identity, KernelSocketState::Unconnected, cloexec, resumed.as_ref())?;
                                self.validate_fd(fd)?;
                            }
                        }
                    } else if call == KernelCall::Socketpair { kernel_output_pair(fields.get(3)?)?; }
                },
                KernelCall::Bind | KernelCall::Connect => {
                    fields.arity(3)?;
                    let fd = kernel_fd(fields.get(0)?)?.fd;
                    let endpoint = kernel_endpoint(fields.get(1)?, kernel_u32(fields.get(2)?)?)?;
                    if call == KernelCall::Bind {
                        if returned >= 0 {
                            if returned != 0 { return Err("KERNEL_BIND_RESULT"); }
                            let (generation, socket) = self.socket_for(fd)?;
                            if socket.state != KernelSocketState::Unconnected { return Err("KERNEL_BIND_ROLE_TRANSITION"); }
                            if socket.class == KernelSocketClass::Tcp4 && endpoint == KernelEndpoint::Listener {
                                if self.target_bound_generation != 0 && self.target_bound_generation != generation {
                                    return Err("KERNEL_LISTENER_NOT_UNIQUE");
                                }
                                self.target_bound_generation = generation;
                                self.set_socket_state(generation, KernelSocketState::BoundTarget)?;
                            } else { self.set_socket_state(generation, KernelSocketState::BoundOther)?; }
                        }
                    } else {
                        if returned > 0 { return Err("KERNEL_CONNECT_RESULT"); }
                        let (generation, socket) = self.socket_for(fd)?;
                        if matches!(socket.state, KernelSocketState::TargetListener | KernelSocketState::AcceptedTarget | KernelSocketState::OtherListener) {
                            return Err("KERNEL_CONNECT_ROLE_TRANSITION");
                        }
                        let state = if socket.class == KernelSocketClass::NonTarget {
                            KernelSocketState::OtherConnected
                        } else if socket.class == KernelSocketClass::Tcp4 && endpoint == KernelEndpoint::Backend && !result.restarted {
                            if returned == 0 { KernelSocketState::BackendConnected }
                            else if result.errno == KernelErrno::InProgress { KernelSocketState::BackendPending }
                            else { KernelSocketState::Unconnected }
                        } else { KernelSocketState::Unconnected };
                        // Every connect result replaces the prior connect claim, including
                        // failed, interrupted, EALREADY and EISCONN attempts. No stale exemption.
                        self.set_socket_state(generation, state)?;
                    }
                },
                KernelCall::Listen => {
                    fields.arity(2)?;
                    let fd = kernel_fd(fields.get(0)?)?.fd;
                    kernel_i32(fields.get(1)?)?;
                    if returned >= 0 {
                        if returned != 0 { return Err("KERNEL_LISTEN_RESULT"); }
                        let (generation, socket) = self.socket_for(fd)?;
                        match socket.state {
                            KernelSocketState::BoundTarget | KernelSocketState::TargetListener if socket.class == KernelSocketClass::Tcp4 => {
                                if self.target_bound_generation != generation ||
                                    (self.target_listener_generation != 0 && self.target_listener_generation != generation) {
                                    return Err("KERNEL_LISTENER_NOT_UNIQUE");
                                }
                                self.target_listener_generation = generation;
                                self.set_socket_state(generation, KernelSocketState::TargetListener)?;
                            },
                            KernelSocketState::Unconnected | KernelSocketState::BoundOther | KernelSocketState::OtherListener => {
                                self.set_socket_state(generation, KernelSocketState::OtherListener)?;
                            },
                            _ => return Err("KERNEL_LISTEN_ROLE_TRANSITION"),
                        }
                    }
                },
                KernelCall::Accept | KernelCall::Accept4 => {
                    fields.arity(if call == KernelCall::Accept { 3 } else { 4 })?;
                    let fd = kernel_fd(fields.get(0)?)?.fd;
                    kernel_accept_address(fields.get(1)?)?;
                    kernel_socklen_output(fields.get(2)?)?;
                    let flags = if call == KernelCall::Accept4 {
                        kernel_named_flags(fields.get(3)?, &[("SOCK_CLOEXEC", 1), ("SOCK_NONBLOCK", 2)])?
                    } else { 0 };
                    if returned >= 0 {
                        let (_, socket) = self.socket_for(fd).map_err(|_| "ACCEPT_SOCKET_UNCLASSIFIED")?;
                        let (class, state) = if socket.class == KernelSocketClass::Tcp4 && socket.state == KernelSocketState::TargetListener {
                            (KernelSocketClass::Tcp4, KernelSocketState::AcceptedTarget)
                        } else if socket.class == KernelSocketClass::NonTarget && socket.state == KernelSocketState::OtherListener {
                            (KernelSocketClass::NonTarget, KernelSocketState::OtherConnected)
                        } else { return Err("ACCEPT_SOCKET_UNCLASSIFIED"); };
                        let target = kernel_return_fd(returned)?;
                        self.birth_socket(target, class, (socket.family_id, socket.type_id, socket.protocol_id),
                            state, flags & 1 != 0, resumed.as_ref())?;
                        self.validate_annotation(target, result.annotation)?;
                        if state == KernelSocketState::AcceptedTarget {
                            self.target_accept_count = self.target_accept_count.checked_add(1).ok_or("KERNEL_ACCEPT_COUNT_LIMIT")?;
                        }
                    }
                },
                KernelCall::Dup | KernelCall::Dup2 | KernelCall::Dup3 => {
                    fields.arity(match call { KernelCall::Dup => 1, KernelCall::Dup2 => 2, _ => 3 })?;
                    let source = kernel_fd(fields.get(0)?)?.fd;
                    let destination = if call == KernelCall::Dup { None } else { Some(kernel_fd(fields.get(1)?)?.fd) };
                    let cloexec = call == KernelCall::Dup3 && kernel_named_flags(fields.get(2)?, &[("O_CLOEXEC", 1)])? != 0;
                    if returned >= 0 {
                        if source < 0 { return Err("KERNEL_DUP_SOURCE"); }
                        let target = kernel_return_fd(returned)?;
                        if destination.is_some_and(|fd| fd != target) { return Err("KERNEL_DUP_RESULT"); }
                        let kind = self.slots.get(&source).and_then(|slot| slot.live).ok_or("KERNEL_FD_SOURCE_ORIGIN_UNKNOWN")?;
                        if source == target {
                            if call != KernelCall::Dup2 { return Err("KERNEL_DUP_SAME_FD"); }
                        } else {
                            if call == KernelCall::Dup { self.check_new_fd(target, resumed.as_ref())?; }
                            self.install(target, kind, cloexec, call != KernelCall::Dup, &mut event)?;
                        }
                        self.validate_annotation(target, result.annotation)?;
                    }
                },
                KernelCall::Fcntl => {
                    let fd = kernel_fd(fields.get(0)?)?.fd;
                    let command = fields.get(1)?;
                    let duplicate = matches!(command, "F_DUPFD" | "F_DUPFD_CLOEXEC");
                    if returned>=0 && !duplicate && self.slots.get(&fd).and_then(|v|v.live).is_none(){return Err("KERNEL_FCNTL_ORIGIN_UNKNOWN");}
                    if (!duplicate && result.annotation.is_some()) ||
                        (result.flags_suffix != 0 && !matches!((command, result.flags_suffix), ("F_GETFD", 1) | ("F_GETFL", 2))) {
                        return Err("KERNEL_FCNTL_RESULT_SUFFIX");
                    }
                    match fields.get(1)? {
                        "F_DUPFD" | "F_DUPFD_CLOEXEC" => {
                            fields.arity(3)?;
                            let minimum = kernel_i32(fields.get(2)?)?;
                            if returned >= 0 {
                                let target = kernel_return_fd(returned)?;
                                if fd < 0 || minimum < 0 || target < minimum || target == fd { return Err("KERNEL_FCNTL_DUP_RESULT"); }
                                let kind = self.slots.get(&fd).and_then(|slot| slot.live).ok_or("KERNEL_FD_SOURCE_ORIGIN_UNKNOWN")?;
                                self.check_new_fd(target, resumed.as_ref())?;
                                self.install(target, kind, fields.get(1)? == "F_DUPFD_CLOEXEC", false, &mut event)?;
                                self.validate_annotation(target, result.annotation)?;
                            }
                        },
                        "F_GETFD" => {
                            fields.arity(2)?;
                            if returned >= 0 {
                                if returned > 1 { return Err("KERNEL_FCNTL_FD_FLAGS"); }
                                if let Some(slot) = self.slots.get(&fd).filter(|slot| slot.live.is_some()) {
                                    if slot.cloexec != (returned == 1) { return Err("KERNEL_CLOEXEC_CONFLICT"); }
                                }
                            }
                        },
                        "F_SETFD" => {
                            fields.arity(3)?;
                            let flags = kernel_named_flags(fields.get(2)?, &[("FD_CLOEXEC", 1)])?;
                            if returned >= 0 {
                                if returned != 0 || fd < 0 { return Err("KERNEL_FCNTL_SET_RESULT"); }
                                self.set_cloexec(fd, flags != 0)?;
                            }
                        },
                        "F_GETFL" => { fields.arity(2)?; },
                        // SQLite startup lock commands do not create/replace FDs.
                        "F_SETLK" | "F_SETLKW" | "F_GETLK" | "F_SETLK64" | "F_SETLKW64" | "F_GETLK64" | "F_OFD_SETLK" | "F_OFD_SETLKW" | "F_OFD_GETLK" => {
                            fields.arity(3)?;if returned>0{return Err("KERNEL_FCNTL_LOCK_RESULT");}
                            kernel_lock_argument(fields.get(2)?,command,returned)?;
                        },
                        "F_SETFL" => {
                            fields.arity(3)?; kernel_open_flags(fields.get(2)?)?;
                            if returned > 0 { return Err("KERNEL_FCNTL_SET_RESULT"); }
                        },
                        _ => return Err("KERNEL_FCNTL_UNSUPPORTED"),
                    }
                },
                KernelCall::Close => {
                    fields.arity(1)?;
                    if returned != 0 || result.restarted { return Err("KERNEL_CLOSE_FAILED"); }
                    let fd = kernel_fd(fields.get(0)?)?.fd;
                    if fd < 0 { return Err("KERNEL_CLOSE_FD"); }
                    self.close_slot(fd, &mut event)?;
                },
                KernelCall::CloseRange => {
                    fields.arity(3)?;
                    if returned != 0 || result.restarted { return Err("KERNEL_CLOSE_RANGE_FAILED"); }
                    let (first, last) = entry.range.ok_or("KERNEL_CLOSE_RANGE_ORDER")?;
                    if first <= i32::MAX as u32 {
                        let mut cursor = first as i32;
                        let end = last.min(i32::MAX as u32) as i32;
                        loop {
                            let next = self.slots.range(cursor..=end).next().map(|(&fd, slot)| (fd, slot.live.is_some()));
                            let Some((fd, live)) = next else { break; };
                            if live {
                                if entry.flags & 4 != 0 { self.set_cloexec(fd, true)?; }
                                else { self.close_slot(fd, &mut event)?; }
                            }
                            if fd == end { break; }
                            let Some(after) = fd.checked_add(1) else { break; }; cursor = after;
                        }
                    }
                },
                KernelCall::Open | KernelCall::Openat | KernelCall::Openat2 | KernelCall::Creat |
                KernelCall::Eventfd | KernelCall::Eventfd2 | KernelCall::EpollCreate | KernelCall::EpollCreate1 |
                KernelCall::TimerfdCreate => {
                    let cloexec = kernel_non_socket_spec(call, &fields)?;
                    if returned >= 0 {
                        let fd = kernel_return_fd(returned)?;
                        self.check_new_fd(fd, resumed.as_ref())?;
                        self.install(fd, KernelFdKind::NonSocket, cloexec, false, &mut event)?;
                        self.validate_annotation(fd, result.annotation)?;
                    }
                },
                KernelCall::Pipe | KernelCall::Pipe2 => {
                    fields.arity(if call == KernelCall::Pipe { 1 } else { 2 })?;
                    let flags = if call == KernelCall::Pipe2 {
                        kernel_named_flags(fields.get(1)?, &[("O_CLOEXEC", 1), ("O_NONBLOCK", 2), ("O_DIRECT", 4), ("O_NOTIFICATION_PIPE", 8)])?
                    } else { 0 };
                    if returned >= 0 {
                        if returned != 0 { return Err("KERNEL_PIPE_RESULT"); }
                        let pair = kernel_fd_pair(fields.get(0)?)?;
                        for fd in pair {
                            self.check_new_fd(fd.fd, resumed.as_ref())?;
                            self.install(fd.fd, KernelFdKind::NonSocket, flags & 1 != 0, false, &mut event)?;
                            self.validate_fd(fd)?;
                        }
                    } else { kernel_output_pair(fields.get(0)?)?; }
                },
                KernelCall::Signalfd | KernelCall::Signalfd4 => {
                    fields.arity(if call == KernelCall::Signalfd { 3 } else { 4 })?;
                    let fd = kernel_fd(fields.get(0)?)?.fd;
                    kernel_signal_set(fields.get(1)?)?;
                    if kernel_u64(fields.get(2)?)? != 8 { return Err("KERNEL_SIGNALSET_SIZE"); }
                    let flags = if call == KernelCall::Signalfd4 {
                        kernel_named_flags(fields.get(3)?, &[("SFD_CLOEXEC", 1), ("SFD_NONBLOCK", 2)])?
                    } else { 0 };
                    if returned >= 0 {
                        let target = kernel_return_fd(returned)?;
                        if fd == -1 {
                            self.check_new_fd(target, resumed.as_ref())?;
                            self.install(target, KernelFdKind::NonSocket, flags & 1 != 0, false, &mut event)?;
                        } else if fd < 0 || target != fd ||
                            self.slots.get(&fd).and_then(|slot| slot.live) != Some(KernelFdKind::NonSocket) {
                            return Err("KERNEL_SIGNALFD_UPDATE_ORIGIN");
                        }
                        self.validate_annotation(target, result.annotation)?;
                    }
                },
                KernelCall::Clone | KernelCall::Clone3 => {
                    if returned == 0 || returned > u32::MAX as i64 { return Err("KERNEL_CLONE_RESULT"); }
                    if returned > 0 {
                        self.shared_clone_count = self.shared_clone_count.checked_add(1).ok_or("KERNEL_CLONE_COUNT_LIMIT")?;
                    }
                },
                _ => return Err("KERNEL_SYSCALL_UNSUPPORTED"),
            }
            Ok(event)
        }
    fn ensure_slot_capacity(&self, fd: i32) -> Result<()> {
        if fd < 0 { return Err("KERNEL_FD_NUMBER"); }
        if !self.slots.contains_key(&fd) && self.slots.len() >= KERNEL_FD_SLOT_LIMIT { return Err("KERNEL_FD_SLOT_LIMIT"); }
        Ok(())
    }
    fn ensure_fd_mutation(&self, fd: i32) -> Result<()> {
        for entry in self.pending.values() {
            if entry.inputs[..entry.input_count].iter().any(|input| input.fd == fd) ||
                entry.range.is_some_and(|(first, last)| (fd as u32) >= first && (fd as u32) <= last) || entry.call.exec() {
                return Err("KERNEL_FD_PENDING_OVERLAP");
            }
        }
        Ok(())
    }
    fn check_new_fd(&self, fd: i32, entry: Option<&KernelFdEntry>) -> Result<()> {
        self.ensure_slot_capacity(fd)?;
        self.ensure_fd_mutation(fd)?;
        if let Some(slot) = self.slots.get(&fd) {
            if slot.live.is_some() { return Err("FD_REUSE_WITHOUT_CLOSE"); }
            if entry.is_some_and(|entry| slot.epoch > entry.sequence) { return Err("KERNEL_RETURN_FD_ENTRY_REUSE"); }
        }
        Ok(())
    }
    fn birth_socket(&mut self, fd: i32, class: KernelSocketClass, identity: (u8, u8, u8),
        state: KernelSocketState, cloexec: bool, entry: Option<&KernelFdEntry>) -> Result<()> {
        self.check_new_fd(fd, entry)?;
        if self.sockets.len() >= KERNEL_FD_SLOT_LIMIT { return Err("KERNEL_SOCKET_LIMIT"); }
        let generation = self.next_socket.checked_add(1).ok_or("KERNEL_SOCKET_GENERATION_LIMIT")?;
        let epoch = self.advance()?;
        self.next_socket = generation;
        self.socket_birth_count = self.socket_birth_count.checked_add(1).ok_or("KERNEL_SOCKET_COUNT_LIMIT")?;
        self.sockets.insert(generation, KernelSocketOrigin { class, family_id: identity.0,
            type_id: identity.1, protocol_id: identity.2, state, version: epoch, aliases: 1 });
        self.slots.insert(fd, KernelFdSlot { epoch, live: Some(KernelFdKind::Socket(generation)), cloexec });
        Ok(())
    }
    fn retire_alias(&mut self, kind: KernelFdKind, event: &mut KernelFdEvent) -> Result<()> {
        let KernelFdKind::Socket(generation) = kind else { return Ok(()); };
        let socket = self.sockets.get_mut(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
        socket.aliases = socket.aliases.checked_sub(1).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
        if socket.aliases == 0 {
            let target = socket.state == KernelSocketState::AcceptedTarget;
            self.sockets.remove(&generation);
            if target {
                if event.retired_targets.len() >= KERNEL_FD_SLOT_LIMIT { return Err("KERNEL_RETIRED_TARGET_LIMIT"); }
                event.retired_targets.push(generation);
                self.retired_target_count = self.retired_target_count.checked_add(1).ok_or("KERNEL_RETIRED_COUNT_LIMIT")?;
            }
        }
        Ok(())
    }
    fn install(&mut self, fd: i32, kind: KernelFdKind, cloexec: bool, replace: bool, event: &mut KernelFdEvent) -> Result<()> {
        self.ensure_slot_capacity(fd)?;
        self.ensure_fd_mutation(fd)?;
        let old = self.slots.get(&fd).and_then(|slot| slot.live);
        if old.is_some() && !replace { return Err("FD_REUSE_WITHOUT_CLOSE"); }
        // Add before subtracting: replacing an alias by another alias of the same
        // socket must never transiently retire that socket generation.
        if let KernelFdKind::Socket(generation) = kind {
            let socket = self.sockets.get_mut(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
            socket.aliases = socket.aliases.checked_add(1).ok_or("KERNEL_SOCKET_ALIAS_LIMIT")?;
        }
        if let Some(old) = old { self.retire_alias(old, event)?; }
        let epoch = self.advance()?;
        self.slots.insert(fd, KernelFdSlot { epoch, live: Some(kind), cloexec });
        Ok(())
    }
    fn close_slot(&mut self, fd: i32, event: &mut KernelFdEvent) -> Result<()> {
        self.ensure_slot_capacity(fd)?;
        self.ensure_fd_mutation(fd)?;
        let kind=self.slots.get(&fd).and_then(|slot|slot.live).ok_or("KERNEL_CLOSE_ORIGIN_UNKNOWN")?;
        self.retire_alias(kind,event)?;
        let epoch = self.advance()?;
        self.slots.insert(fd, KernelFdSlot { epoch, live: None, cloexec: false });
        Ok(())
    }
    fn set_cloexec(&mut self, fd: i32, cloexec: bool) -> Result<()> {
        self.ensure_slot_capacity(fd)?;
        self.ensure_fd_mutation(fd)?;
        let live = self.slots.get(&fd).and_then(|slot| slot.live).ok_or("KERNEL_FD_SOURCE_ORIGIN_UNKNOWN")?;
        let epoch = self.advance()?;
        self.slots.insert(fd, KernelFdSlot { epoch, live: Some(live), cloexec });
        Ok(())
    }
    fn set_socket_state(&mut self, generation: u64, state: KernelSocketState) -> Result<()> {
        if self.sockets.get(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?.state == state { return Ok(()); }
        for entry in self.pending.values() {
            if entry.inputs[..entry.input_count].iter().any(|input| input.socket_generation == generation) {
                return Err("KERNEL_SOCKET_PENDING_OVERLAP");
            }
            if let Some((first, last)) = entry.range {
                if self.slots.iter().any(|(&fd, slot)| (fd as u32) >= first && (fd as u32) <= last &&
                    slot.live == Some(KernelFdKind::Socket(generation))) { return Err("KERNEL_SOCKET_PENDING_OVERLAP"); }
            }
        }
        let version = self.advance()?;
        let socket = self.sockets.get_mut(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
        socket.state = state; socket.version = version;
        Ok(())
    }
    fn validate_fd(&self, fd: KernelParsedFd<'_>) -> Result<()> { self.validate_annotation(fd.fd, fd.annotation) }
    fn validate_annotation(&self, fd: i32, annotation: Option<&str>) -> Result<()> {
        let Some(annotation) = annotation else { return Ok(()); };
        let parsed = kernel_annotation(annotation)?;
        let Some(kind) = self.slots.get(&fd).and_then(|slot| slot.live) else { return Ok(()); };
        match kind {
            KernelFdKind::NonSocket => {
                if parsed.socket { return Err("FD_IDENTITY_CONFLICT"); }
            },
            KernelFdKind::Socket(generation) => {
                if !parsed.socket { return Err("FD_IDENTITY_CONFLICT"); }
                let socket = self.sockets.get(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
                if socket.class == KernelSocketClass::Tcp4 && parsed.protocol != 0 && parsed.protocol != 1 {
                    return Err("FD_IDENTITY_CONFLICT");
                }
                if parsed.complete {
                    match socket.state {
                        KernelSocketState::BoundTarget | KernelSocketState::TargetListener |
                        KernelSocketState::AcceptedTarget if !parsed.listener_local => return Err("FD_IDENTITY_CONFLICT"),
                        KernelSocketState::BackendPending | KernelSocketState::BackendConnected if !parsed.backend_peer => return Err("FD_IDENTITY_CONFLICT"),
                        KernelSocketState::Unconnected => return Err("FD_IDENTITY_CONFLICT"),
                        _ => {},
                    }
                    if socket.state == KernelSocketState::AcceptedTarget && !parsed.connected { return Err("FD_IDENTITY_CONFLICT"); }
                    if matches!(socket.state, KernelSocketState::BoundTarget | KernelSocketState::TargetListener) &&
                        parsed.connected { return Err("FD_IDENTITY_CONFLICT"); }
                    if socket.class == KernelSocketClass::NonTarget && parsed.protocol == 1 { return Err("FD_IDENTITY_CONFLICT"); }
                }
            },
        }
        Ok(())
    }
    fn target_role(&self, fd: i32) -> Result<Option<u64>> {
        self.usable()?;
        match self.slots.get(&fd).and_then(|slot| slot.live) {
            Some(KernelFdKind::NonSocket) => Ok(None),
            Some(KernelFdKind::Socket(generation)) => {
                let socket = self.sockets.get(&generation).ok_or("KERNEL_SOCKET_ALIAS_INVARIANT")?;
                if !self.exec_confirmed { return Err("IO_SOCKET_UNCLASSIFIED"); }
                if socket.class == KernelSocketClass::NonTarget { return Ok(None); }
                if socket.class == KernelSocketClass::Tcp4 {
                    match socket.state {
                        KernelSocketState::AcceptedTarget => return Ok(Some(generation)),
                        KernelSocketState::BackendPending | KernelSocketState::BackendConnected => return Ok(None),
                        _ => {},
                    }
                }
                Err("IO_SOCKET_UNCLASSIFIED")
            },
            _ => Err("IO_SOCKET_UNCLASSIFIED"),
        }
    }
    fn listener_ready(&self) -> bool {
        !self.failed && self.exec_confirmed && !self.awaiting_exec_confirmation && self.resumed.is_none() &&
            self.target_listener_generation != 0 && self.sockets.get(&self.target_listener_generation)
                .is_some_and(|socket| socket.class == KernelSocketClass::Tcp4 &&
                    socket.state == KernelSocketState::TargetListener && socket.aliases > 0) &&
            self.pending.values().all(|entry| entry.call.io() || matches!(entry.call, KernelCall::Accept | KernelCall::Accept4))
    }
    fn projection(&self) -> Value {
        let fds = self.slots.iter().filter_map(|(&fd, slot)| slot.live.map(|kind| {
            let (origin_id, generation) = match kind {
                KernelFdKind::NonSocket => (1u8, 0),
                KernelFdKind::Socket(generation) => (3, generation),
            };
            json!({"fd":fd,"descriptor_epoch":slot.epoch,"origin_id":origin_id,
                "socket_generation":generation,"cloexec":slot.cloexec})
        })).collect::<Vec<_>>();
        let sockets = self.sockets.iter().map(|(&generation, socket)| json!({
            "generation":generation,"class_id":socket.class.id(),"role_id":socket.state.id(),
            "family_id":socket.family_id,"type_id":socket.type_id,"protocol_id":socket.protocol_id,
            "state_epoch":socket.version,"alias_count":socket.aliases
        })).collect::<Vec<_>>();
        let pending = self.pending.iter().map(|(&tid, entry)| json!({
            "tid":tid,"call_id":entry.call.id(),"entry_epoch":entry.sequence,
            "input_count":entry.input_count,"input_fds":entry.inputs[..entry.input_count].iter().map(|input| json!({
                "fd":input.fd,"descriptor_epoch":input.epoch,"socket_generation":input.socket_generation,
                "socket_state_epoch":input.socket_version
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>();
        json!({"origin_method_id":1,"initial_non_socket_fd_count":self.initial_fd_count,
            "slot_limit":KERNEL_FD_SLOT_LIMIT,"pending_limit":KERNEL_FD_PENDING_LIMIT,
            "historical_slot_count":self.slots.len(),"event_count":self.event_count,
            "socket_birth_count":self.socket_birth_count,"target_accept_count":self.target_accept_count,
            "retired_target_count":self.retired_target_count,"shared_clone_count":self.shared_clone_count,
            "raw_exec_started":self.exec_started,"raw_exec_succeeded":self.exec_succeeded,
            "published_exec_confirmation_received":self.exec_confirmed,
            "exec_confirmation_required":self.awaiting_exec_confirmation,
            "target_bound_generation":self.target_bound_generation,"target_listener_generation":self.target_listener_generation,
            "listener_ready":self.listener_ready(),"failed":self.failed,"resume_awaits_event":self.resumed.is_some(),
            "fds":fds,"sockets":sockets,"pending":pending,
            "annotation_identity_fallback":false,"payload_identity_fallback":false,"proc_identity_fallback":false,
            "raw_arguments_addresses_annotations_paths_errors_exported":false})
    }
}

// Fixed-capacity, borrowing grammar: quoted buffers, paths and annotations are
// skipped structurally, never copied into model state or into diagnostics.
struct KernelFields<'a> { values: [&'a str; 16], len: usize }
impl<'a> KernelFields<'a> {
    fn get(&self, at: usize) -> Result<&'a str> {
        if at >= self.len || self.values[at].is_empty() { return Err("KERNEL_ARGUMENT_MISSING"); }
        Ok(self.values[at])
    }
    fn arity(&self, count: usize) -> Result<()> {
        if self.len != count || self.values[..self.len].iter().any(|field| field.is_empty()) { return Err("KERNEL_ARGUMENT_COUNT"); }
        Ok(())
    }
    fn push(&mut self, value: &'a str) -> Result<()> {
        if self.len == self.values.len() { return Err("KERNEL_ARGUMENT_LIMIT"); }
        self.values[self.len] = value.trim(); self.len += 1; Ok(())
    }
}
fn kernel_fields(text: &str, prefix: bool) -> Result<KernelFields<'_>> {
    let mut fields = KernelFields { values: [""; 16], len: 0 };
    if text.trim().is_empty() { return Ok(fields); }
    let bytes = text.as_bytes(); let mut stack = [0u8; 16]; let mut depth = 0usize;
    let mut quote = false; let mut angle = false; let mut escaped = false; let mut start = 0usize;
    for (at, &byte) in bytes.iter().enumerate() {
        if angle {
            if byte == b'>' && (at == 0 || bytes[at - 1] != b'-') { angle = false; }
            continue;
        }
        if quote {
            if escaped { escaped = false; }
            else if byte == b'\\' { escaped = true; }
            else if byte == b'"' { quote = false; }
            continue;
        }
        match byte {
            b'"' => quote = true,
            b'<' => angle = true,
            b'(' | b'[' | b'{' => {
                if depth == stack.len() { return Err("KERNEL_ARGUMENT_DEPTH"); }
                stack[depth] = byte; depth += 1;
            },
            b')' | b']' | b'}' => {
                let expected = match byte { b')' => b'(', b']' => b'[', _ => b'{' };
                if depth == 0 || stack[depth - 1] != expected { return Err("KERNEL_ARGUMENT_DELIMITER"); }
                depth -= 1;
            },
            b',' if depth == 0 => { fields.push(&text[start..at])?; start = at + 1; },
            b'\r' | b'\n' | 0 => return Err("KERNEL_ARGUMENT_SYNTAX"),
            _ => {},
        }
    }
    if !prefix && (depth != 0 || quote || angle || escaped) { return Err("KERNEL_ARGUMENT_TRUNCATED"); }
    fields.push(&text[start..])?;
    if !prefix && fields.values[..fields.len].iter().any(|value| value.is_empty()) { return Err("KERNEL_ARGUMENT_MISSING"); }
    Ok(fields)
}
fn kernel_u64(text: &str) -> Result<u64> {
    if let Some(hex) = text.strip_prefix("0x") {
        if hex.is_empty() || hex.len() > 16 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err("KERNEL_NUMBER"); }
        u64::from_str_radix(hex, 16).map_err(|_| "KERNEL_NUMBER")
    } else {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) { return Err("KERNEL_NUMBER"); }
        text.parse().map_err(|_| "KERNEL_NUMBER")
    }
}
fn kernel_u32(text: &str) -> Result<u32> { u32::try_from(kernel_u64(text)?).map_err(|_| "KERNEL_NUMBER") }
fn kernel_i32(text: &str) -> Result<i32> {
    if let Some(rest) = text.strip_prefix('-') {
        if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) { return Err("KERNEL_NUMBER"); }
        text.parse().map_err(|_| "KERNEL_NUMBER")
    } else { i32::try_from(kernel_u64(text)?).map_err(|_| "KERNEL_NUMBER") }
}
fn kernel_return_fd(returned: i64) -> Result<i32> { i32::try_from(returned).ok().filter(|fd| *fd >= 0).ok_or("KERNEL_RETURN_FD") }
#[derive(Clone, Copy)]
struct KernelParsedFd<'a> { fd: i32, annotation: Option<&'a str> }
fn kernel_fd(text: &str) -> Result<KernelParsedFd<'_>> {
    let (number, annotation) = if let Some((number, _)) = text.split_once('<') {
        (number, Some(kernel_angle(&text[number.len()..])?))
    } else { (text, None) };
    let fd = kernel_i32(number)?;
    if fd < -1 || (fd < 0 && annotation.is_some()) { return Err("KERNEL_FD_NUMBER"); }
    Ok(KernelParsedFd { fd, annotation })
}
fn kernel_dirfd(text: &str) -> Result<KernelParsedFd<'_>> {
    for prefix in ["AT_FDCWD", "-100"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let annotation = if rest.is_empty() { None } else { Some(kernel_angle(rest)?) };
            if annotation.is_some_and(|text| kernel_annotation(text).map_or(true, |value| value.socket)) {
                return Err("KERNEL_DIRECTORY_ANNOTATION");
            }
            return Ok(KernelParsedFd { fd: -100, annotation });
        }
    }
    kernel_fd(text)
}
fn kernel_angle(text: &str) -> Result<&str> {
    let inner = text.strip_prefix('<').and_then(|text| text.strip_suffix('>')).ok_or("KERNEL_ANGLE_WRAP")?;
    // strace -yy appends a device-class tag inside the annotation for character and
    // block devices, e.g. "/dev/null<char 1:3>". Strip exactly that documented
    // suffix so the remaining annotation is the plain target; any other nesting is
    // still rejected below.
    let inner = strip_device_tag(inner).unwrap_or(inner);
    if inner.is_empty() { return Err("KERNEL_ANGLE_EMPTY"); }
    if inner.contains('<') { return Err("KERNEL_ANGLE_NESTED"); }
    if inner.bytes().enumerate().any(|(at, byte)|
        byte == b'>' && (at == 0 || inner.as_bytes()[at - 1] != b'-')) { return Err("KERNEL_ANGLE_LT_GT"); }
    Ok(inner)
}
fn strip_device_tag(inner: &str) -> Option<&str> {
    let (head, tag) = inner.rsplit_once('<')?;
    let (class, numbers) = tag.strip_suffix('>')?.split_once(' ')?;
    if class != "char" && class != "block" { return None; }
    let (major, minor) = numbers.split_once(':')?;
    if head.is_empty() || major.is_empty() || minor.is_empty()
        || !major.bytes().all(|byte| byte.is_ascii_digit())
        || !minor.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
    Some(head)
}
fn kernel_pointer(text: &str) -> Result<()> {
    if text == "NULL" { return Ok(()); }
    let Some(hex) = text.strip_prefix("0x") else { return Err("KERNEL_POINTER"); };
    if hex.is_empty() || hex.len() > 16 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err("KERNEL_POINTER"); }
    Ok(())
}
fn kernel_named_flags(text: &str, names: &[(&str, u64)]) -> Result<u64> {
    if text == "0" { return Ok(0); }
    let mut flags = 0u64;
    for name in text.split('|') {
        let bit = names.iter().find_map(|&(candidate, bit)| (candidate == name).then_some(bit)).ok_or("KERNEL_FLAG_UNSUPPORTED")?;
        if flags & bit != 0 { return Err("KERNEL_FLAG_DUPLICATE"); }
        flags |= bit;
    }
    if flags == 0 { return Err("KERNEL_FLAG_UNSUPPORTED"); }
    Ok(flags)
}
fn kernel_open_flags(text: &str) -> Result<(bool, bool)> {
    const FLAGS: &[(&str, u64)] = &[
        ("O_RDONLY", 1), ("O_WRONLY", 2), ("O_RDWR", 4), ("O_CLOEXEC", 8),
        ("O_CREAT", 16), ("O_EXCL", 32), ("O_NOCTTY", 64), ("O_TRUNC", 128),
        ("O_APPEND", 256), ("O_NONBLOCK", 512), ("O_DSYNC", 1024), ("O_ASYNC", 2048),
        ("O_DIRECT", 4096), ("O_LARGEFILE", 8192), ("O_DIRECTORY", 16384), ("O_NOFOLLOW", 32768),
        ("O_NOATIME", 65536), ("O_SYNC", 131072), ("O_PATH", 262144), ("O_TMPFILE", 524288),
    ];
    let flags = kernel_named_flags(text, FLAGS)?;
    if (flags & 7).count_ones() > 1 { return Err("KERNEL_OPEN_ACCESS_FLAGS"); }
    Ok((flags & 8 != 0, flags & (16 | 524288) != 0))
}
fn kernel_socket_spec(family: &str, kind: &str, protocol: &str) -> Result<(KernelSocketClass, bool, (u8, u8, u8))> {
    let mut base = None; let mut cloexec = false; let mut nonblock = false;
    for token in kind.split('|') {
        match token {
            "SOCK_CLOEXEC" if !cloexec => cloexec = true,
            "SOCK_NONBLOCK" if !nonblock => nonblock = true,
            "SOCK_CLOEXEC" | "SOCK_NONBLOCK" => return Err("KERNEL_FLAG_DUPLICATE"),
            value if base.is_none() && kernel_symbol(value, "SOCK_") => base = Some(value),
            value if base.is_none() => { kernel_unknown_scalar(value, "SOCK_???")?; base = Some(value); },
            _ => return Err("KERNEL_SOCKET_TYPE_SYNTAX"),
        }
    }
    let base = base.ok_or("KERNEL_SOCKET_TYPE_SYNTAX")?;
    let type_id = match base {
        "SOCK_STREAM" => 1, "SOCK_DGRAM" => 2, "SOCK_RAW" => 3, "SOCK_RDM" => 4,
        "SOCK_SEQPACKET" => 5, "SOCK_DCCP" => 6, "SOCK_PACKET" => 7, _ => 0,
    };
    let family_id = match family {
        "AF_INET" => 1, "AF_INET6" => 2, "AF_UNIX" | "AF_LOCAL" => 3,
        "AF_NETLINK" => 4, "AF_PACKET" => 5, _ => {
            if !kernel_symbol(family, "AF_") { kernel_unknown_scalar(family, "AF_???")?; } 0
        },
    };
    let (protocol_number, protocol_id) = match protocol {
        "IPPROTO_IP" => (Some(0), 1), "IPPROTO_TCP" => (Some(6), 2),
        "IPPROTO_UDP" => (Some(17), 3), "IPPROTO_ICMP" => (Some(1), 4),
        "IPPROTO_ICMPV6" => (Some(58), 5), "IPPROTO_RAW" => (Some(255), 6),
        "NETLINK_ROUTE" => (Some(0), 7), "NETLINK_USERSOCK" => (Some(2), 8), "NETLINK_SOCK_DIAG" => (Some(4), 9),
        value if family_id == 5 && value.starts_with("htons(") => {
            let inner = value.strip_prefix("htons(").and_then(|text| text.strip_suffix(')')).ok_or("KERNEL_SOCKET_PROTOCOL")?;
            if !kernel_symbol(inner, "ETH_P_") && kernel_unknown_scalar(inner, "ETH_P_???")? > u16::MAX as u32 {
                return Err("KERNEL_SOCKET_PROTOCOL");
            }
            (None, 10)
        },
        value if kernel_symbol(value, "") && value.as_bytes().first().is_some_and(|byte| byte.is_ascii_uppercase()) => (None, 0),
        value => {
            let number = kernel_unknown_scalar(value, if family_id == 4 { "NETLINK_???" } else { "IPPROTO_???" })?;
            let id = match number { 0 => 1, 6 => 2, 17 => 3, 1 => 4, 58 => 5, 255 => 6, _ => 0 };
            (Some(number), id)
        },
    };
    let class = if family_id == 1 && type_id == 1 &&
        matches!(protocol_number, Some(0 | 6)) && !protocol.starts_with("NETLINK_") {
        KernelSocketClass::Tcp4
    } else if type_id != 0 && ((3..=5).contains(&family_id) || ((1..=2).contains(&family_id) && type_id != 1)) {
        KernelSocketClass::NonTarget
    } else { KernelSocketClass::Unknown };
    Ok((class, cloexec, (family_id, type_id, protocol_id)))
}
fn kernel_symbol(text: &str, prefix: &str) -> bool {
    text.len() > prefix.len() && text.len() <= 64 && text.starts_with(prefix) &&
        text.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}
fn kernel_unknown_scalar(text: &str, marker: &str) -> Result<u32> {
    if let Some((number, comment)) = text.split_once(" /* ") {
        if comment.strip_suffix(" */") != Some(marker) { return Err("KERNEL_ENUM_GRAMMAR"); }
        kernel_u32(number)
    } else { kernel_u32(text) }
}
fn kernel_fd_pair(text: &str) -> Result<[KernelParsedFd<'_>; 2]> {
    let inner = text.strip_prefix('[').and_then(|text| text.strip_suffix(']')).ok_or("KERNEL_FD_PAIR")?;
    let fields = kernel_fields(inner, false)?; fields.arity(2)?;
    let pair = [kernel_fd(fields.get(0)?)?, kernel_fd(fields.get(1)?)?];
    if pair[0].fd < 0 || pair[1].fd < 0 || pair[0].fd == pair[1].fd { return Err("KERNEL_FD_PAIR"); }
    Ok(pair)
}
fn kernel_output_pair(text: &str) -> Result<()> {
    if text.starts_with('[') { kernel_fd_pair(text)?; Ok(()) } else { kernel_pointer(text) }
}
fn kernel_mode(text: &str) -> Result<()> {
    if text.is_empty() || !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) { return Err("KERNEL_MODE"); }
    if u32::from_str_radix(text, 8).map_err(|_| "KERNEL_MODE")? > 0o7777 { return Err("KERNEL_MODE"); }
    Ok(())
}
fn kernel_non_socket_spec(call: KernelCall, fields: &KernelFields<'_>) -> Result<bool> {
    match call {
        KernelCall::Open | KernelCall::Openat => {
            let offset = usize::from(call == KernelCall::Openat);
            if offset != 0 { kernel_dirfd(fields.get(0)?)?; }
            kernel_quoted(fields.get(offset)?)?;
            let (cloexec, mode) = kernel_open_flags(fields.get(offset + 1)?)?;
            fields.arity(offset + if mode { 3 } else { 2 })?;
            if mode { kernel_mode(fields.get(offset + 2)?)?; }
            Ok(cloexec)
        },
        KernelCall::Openat2 => {
            fields.arity(4)?; kernel_dirfd(fields.get(0)?)?; kernel_quoted(fields.get(1)?)?;
            let inner = fields.get(2)?.strip_prefix('{').and_then(|text| text.strip_suffix('}')).ok_or("KERNEL_OPEN_HOW")?;
            let how = kernel_fields(inner, false)?;
            let (cloexec, needs_mode) = kernel_open_flags(how.get(0)?.strip_prefix("flags=").ok_or("KERNEL_OPEN_HOW")?)?;
            let resolve_index = if let Some(mode) = how.get(1)?.strip_prefix("mode=") {
                kernel_mode(mode)?; 2
            } else {
                if needs_mode { return Err("KERNEL_OPEN_HOW"); } 1
            };
            how.arity(resolve_index + 1)?;
            kernel_named_flags(how.get(resolve_index)?.strip_prefix("resolve=").ok_or("KERNEL_OPEN_HOW")?, &[
                ("RESOLVE_NO_XDEV", 1), ("RESOLVE_NO_MAGICLINKS", 2), ("RESOLVE_NO_SYMLINKS", 4),
                ("RESOLVE_BENEATH", 8), ("RESOLVE_IN_ROOT", 16), ("RESOLVE_CACHED", 32)])?;
            if kernel_u64(fields.get(3)?)? != 24 { return Err("KERNEL_OPEN_HOW_SIZE"); }
            Ok(cloexec)
        },
        KernelCall::Creat => { fields.arity(2)?; kernel_quoted(fields.get(0)?)?; kernel_mode(fields.get(1)?)?; Ok(false) },
        KernelCall::Eventfd | KernelCall::Eventfd2 => {
            fields.arity(if call == KernelCall::Eventfd { 1 } else { 2 })?;
            kernel_u32(fields.get(0)?)?;
            if call == KernelCall::Eventfd { Ok(false) } else {
                Ok(kernel_named_flags(fields.get(1)?, &[("EFD_CLOEXEC", 1), ("EFD_NONBLOCK", 2), ("EFD_SEMAPHORE", 4)])? & 1 != 0)
            }
        },
        KernelCall::EpollCreate => { fields.arity(1)?; kernel_i32(fields.get(0)?)?; Ok(false) },
        KernelCall::EpollCreate1 => { fields.arity(1)?; Ok(kernel_named_flags(fields.get(0)?, &[("EPOLL_CLOEXEC", 1)])? != 0) },
        KernelCall::TimerfdCreate => {
            fields.arity(2)?;
            match fields.get(0)? {
                "CLOCK_REALTIME" | "CLOCK_MONOTONIC" | "CLOCK_BOOTTIME" | "CLOCK_REALTIME_ALARM" | "CLOCK_BOOTTIME_ALARM" => {},
                _ => return Err("KERNEL_TIMER_CLOCK"),
            }
            Ok(kernel_named_flags(fields.get(1)?, &[("TFD_CLOEXEC", 1), ("TFD_NONBLOCK", 2)])? & 1 != 0)
        },
        _ => Err("KERNEL_NON_SOCKET_CREATOR"),
    }
}

fn kernel_quoted(text: &str) -> Result<()> {
    if !text.starts_with('"') { return Err("KERNEL_QUOTED_STRING"); }
    let bytes = text.as_bytes(); let mut at = 1usize;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => return if at + 1 == bytes.len() { Ok(()) } else { Err("KERNEL_STRING_TRUNCATED") },
            b'\\' => {
                at += 1;
                let escape = *bytes.get(at).ok_or("KERNEL_QUOTED_STRING")?;
                match escape {
                    b'x' => {
                        let pair = bytes.get(at + 1..at + 3).ok_or("KERNEL_STRING_ESCAPE")?;
                        if !pair.iter().all(|byte| byte.is_ascii_hexdigit()) { return Err("KERNEL_STRING_ESCAPE"); }
                        at += 2;
                    },
                    b'0'..=b'7' => {
                        for _ in 0..2 {
                            if bytes.get(at + 1).is_some_and(|byte| (b'0'..=b'7').contains(byte)) { at += 1; } else { break; }
                        }
                    },
                    b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'"' => {},
                    _ => return Err("KERNEL_STRING_ESCAPE"),
                }
            },
            0..=31 | 127 => return Err("KERNEL_QUOTED_STRING"),
            _ => {},
        }
        at += 1;
    }
    Err("KERNEL_QUOTED_STRING")
}
fn kernel_ascii_string(text: &str, output: &mut [u8]) -> Result<usize> {
    kernel_quoted(text)?;
    let bytes = text.as_bytes(); let mut at = 1usize; let mut used = 0usize;
    while at + 1 < bytes.len() {
        let mut byte = bytes[at];
        if byte == b'\\' {
            at += 1;
            match bytes[at] {
                b'x' => {
                    let hex = &text[at + 1..at + 3]; byte = u8::from_str_radix(hex, 16).map_err(|_| "KERNEL_STRING_ESCAPE")?; at += 2;
                },
                b'0'..=b'7' => {
                    let start = at;
                    for _ in 0..2 {
                        if bytes.get(at + 1).is_some_and(|value| (b'0'..=b'7').contains(value)) { at += 1; } else { break; }
                    }
                    byte = u8::from_str_radix(&text[start..=at], 8).map_err(|_| "KERNEL_STRING_ESCAPE")?;
                },
                b'\\' => byte = b'\\', b'"' => byte = b'"',
                _ => return Err("KERNEL_ADDRESS_STRING"),
            }
        }
        if !byte.is_ascii() || byte == 0 || used == output.len() { return Err("KERNEL_ADDRESS_STRING"); }
        output[used] = byte; used += 1; at += 1;
    }
    Ok(used)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelEndpoint { Listener, Backend, Other, Unknown }
fn kernel_wrapped_number(text: &str, prefix: &str) -> Result<u32> {
    kernel_u32(text.strip_prefix(prefix).and_then(|text| text.strip_suffix(')')).ok_or("KERNEL_SOCKADDR_GRAMMAR")?)
}
fn kernel_endpoint(text: &str, length: u32) -> Result<KernelEndpoint> {
    if text == "NULL" || text.starts_with("0x") { kernel_pointer(text)?; return Ok(KernelEndpoint::Unknown); }
    let inner = text.strip_prefix('{').and_then(|text| text.strip_suffix('}')).ok_or("KERNEL_SOCKADDR_GRAMMAR")?;
    let fields = kernel_fields(inner, false)?;
    match fields.get(0)? {
        "sa_family=AF_INET" => {
            fields.arity(3)?;
            let port = kernel_wrapped_number(fields.get(1)?, "sin_port=htons(")?;
            if port > u16::MAX as u32 { return Err("KERNEL_SOCKADDR_PORT"); }
            let address = fields.get(2)?.strip_prefix("sin_addr=inet_addr(").and_then(|text| text.strip_suffix(')')).ok_or("KERNEL_SOCKADDR_GRAMMAR")?;
            let mut buffer = [0u8; 15]; let used = kernel_ascii_string(address, &mut buffer)?;
            let address = std::str::from_utf8(&buffer[..used]).map_err(|_| "KERNEL_SOCKADDR_GRAMMAR")?;
            let parsed = address.parse::<std::net::Ipv4Addr>().map_err(|_| "KERNEL_SOCKADDR_GRAMMAR")?;
            if length != 16 { return Ok(KernelEndpoint::Unknown); }
            if parsed == std::net::Ipv4Addr::new(127, 0, 0, 1) {
                if port == 18634 { return Ok(KernelEndpoint::Listener); }
                if port == 18631 { return Ok(KernelEndpoint::Backend); }
            }
            Ok(KernelEndpoint::Other)
        },
        "sa_family=AF_INET6" => {
            if fields.len != 4 && fields.len != 5 { return Err("KERNEL_SOCKADDR_GRAMMAR"); }
            if kernel_wrapped_number(fields.get(1)?, "sin6_port=htons(")? > u16::MAX as u32 { return Err("KERNEL_SOCKADDR_PORT"); }
            kernel_wrapped_number(fields.get(2)?, "sin6_flowinfo=htonl(")?;
            let address = fields.get(3)?.strip_prefix("inet_pton(AF_INET6, ").and_then(|text| text.strip_suffix(", &sin6_addr)")).ok_or("KERNEL_SOCKADDR_GRAMMAR")?;
            let mut buffer = [0u8; 64]; let used = kernel_ascii_string(address, &mut buffer)?;
            std::str::from_utf8(&buffer[..used]).map_err(|_| "KERNEL_SOCKADDR_GRAMMAR")?.parse::<std::net::Ipv6Addr>().map_err(|_| "KERNEL_SOCKADDR_GRAMMAR")?;
            if fields.len == 5 {
                kernel_u32(fields.get(4)?.strip_prefix("sin6_scope_id=").ok_or("KERNEL_SOCKADDR_GRAMMAR")?)?;
            }
            Ok(KernelEndpoint::Other)
        },
        "sa_family=AF_UNIX" | "sa_family=AF_LOCAL" => {
            if fields.len == 1 { return Ok(KernelEndpoint::Other); }
            fields.arity(2)?;
            let path = fields.get(1)?.strip_prefix("sun_path=").ok_or("KERNEL_SOCKADDR_GRAMMAR")?;
            kernel_quoted(path.strip_prefix('@').unwrap_or(path))?;
            Ok(KernelEndpoint::Other)
        },
        "sa_family=AF_UNSPEC" => {
            if fields.len == 2 { kernel_quoted(fields.get(1)?.strip_prefix("sa_data=").ok_or("KERNEL_SOCKADDR_GRAMMAR")?)?; }
            else { fields.arity(1)?; }
            Ok(KernelEndpoint::Other)
        },
        "sa_family=AF_NETLINK" => {
            fields.arity(3)?;
            kernel_u32(fields.get(1)?.strip_prefix("nl_pid=").ok_or("KERNEL_SOCKADDR_GRAMMAR")?)?;
            kernel_u32(fields.get(2)?.strip_prefix("nl_groups=").ok_or("KERNEL_SOCKADDR_GRAMMAR")?)?;
            Ok(KernelEndpoint::Other)
        },
        _ => Err("KERNEL_SOCKADDR_UNSUPPORTED"),
    }
}
fn kernel_accept_address(text: &str) -> Result<()> {
    // Peer bytes are never an identity input. Parsing only bounds their grammar.
    kernel_endpoint(text, 16)?; Ok(())
}
fn kernel_socklen_output(text: &str) -> Result<()> {
    if text == "NULL" || text.starts_with("0x") { return kernel_pointer(text); }
    let inner = text.strip_prefix('[').and_then(|text| text.strip_suffix(']')).ok_or("KERNEL_SOCKLEN_GRAMMAR")?;
    if let Some((before, after)) = inner.split_once(" => ") { kernel_u32(before)?; kernel_u32(after)?; }
    else { kernel_u32(inner)?; }
    Ok(())
}

const KERNEL_CLONE_FLAGS: &[(&str, u64)] = &[
    ("CLONE_VM", 1), ("CLONE_FS", 2), ("CLONE_FILES", 4), ("CLONE_SIGHAND", 8),
    ("CLONE_THREAD", 16), ("CLONE_SYSVSEM", 32), ("CLONE_SETTLS", 64),
    ("CLONE_PARENT_SETTID", 128), ("CLONE_CHILD_CLEARTID", 256), ("CLONE_CHILD_SETTID", 512), ("CLONE_IO", 1024),
];
fn kernel_checked_clone_flags(text: &str) -> Result<u64> {
    let flags = kernel_named_flags(text, KERNEL_CLONE_FLAGS)?;
    if flags & (1 | 4 | 8 | 16) != (1 | 4 | 8 | 16) { return Err("KERNEL_CLONE_NOT_SHARED_THREAD"); }
    Ok(flags)
}
fn kernel_clone_tid(text: &str) -> Result<()> {
    if text.starts_with('[') {
        let inner = text.strip_prefix('[').and_then(|text| text.strip_suffix(']')).ok_or("KERNEL_CLONE_GRAMMAR")?;
        if kernel_u32(inner)? == 0 { return Err("KERNEL_CLONE_GRAMMAR"); }
        Ok(())
    } else { kernel_pointer(text) }
}
fn kernel_clone_flags(call: KernelCall, fields: &KernelFields<'_>, prefix: bool) -> Result<u64> {
    if call == KernelCall::Clone {
        kernel_pointer(fields.get(0)?.strip_prefix("child_stack=").ok_or("KERNEL_CLONE_GRAMMAR")?)?;
        let flags = kernel_checked_clone_flags(fields.get(1)?.strip_prefix("flags=").ok_or("KERNEL_CLONE_GRAMMAR")?)?;
        if prefix && fields.len == 2 { return Ok(flags); }
        let mut at = 2usize;
        if flags & 128 != 0 { kernel_clone_tid(fields.get(at)?.strip_prefix("parent_tid=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1; }
        if flags & 64 != 0 { kernel_pointer(fields.get(at)?.strip_prefix("tls=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1; }
        if flags & (256 | 512) != 0 { kernel_pointer(fields.get(at)?.strip_prefix("child_tidptr=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1; }
        fields.arity(at)?;
        Ok(flags)
    } else {
        let text = fields.get(0)?;
        let (before, changed) = match text.split_once(" => ") { Some((before, changed)) => (before, Some(changed)), None => (text, None) };
        let inner = before.strip_prefix('{').and_then(|text| text.strip_suffix('}')).ok_or("KERNEL_CLONE3_OPAQUE")?;
        let args = kernel_fields(inner, false)?;
        let flags = kernel_checked_clone_flags(args.get(0)?.strip_prefix("flags=").ok_or("KERNEL_CLONE_GRAMMAR")?)?;
        let mut at = 1usize;
        if flags & (256 | 512) != 0 { kernel_pointer(args.get(at)?.strip_prefix("child_tid=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1; }
        if flags & 128 != 0 { kernel_pointer(args.get(at)?.strip_prefix("parent_tid=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1; }
        if args.get(at)? != "exit_signal=0" { return Err("KERNEL_CLONE_NOT_SHARED_THREAD"); } at += 1;
        kernel_pointer(args.get(at)?.strip_prefix("stack=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1;
        kernel_u64(args.get(at)?.strip_prefix("stack_size=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1;
        if flags & 64 != 0 { kernel_pointer(args.get(at)?.strip_prefix("tls=").ok_or("KERNEL_CLONE_GRAMMAR")?)?; at += 1; }
        args.arity(at)?;
        if let Some(changed) = changed {
            if flags & 128 == 0 { return Err("KERNEL_CLONE_GRAMMAR"); }
            let tid = changed.strip_prefix("{parent_tid=").and_then(|text| text.strip_suffix('}')).ok_or("KERNEL_CLONE_GRAMMAR")?;
            kernel_clone_tid(tid)?;
        }
        if prefix && fields.len == 1 { return Ok(flags); }
        fields.arity(2)?;
        if !matches!(kernel_u64(fields.get(1)?)?, 64 | 80 | 88) { return Err("KERNEL_CLONE3_SIZE"); }
        Ok(flags)
    }
}
fn kernel_raw_word(text: &str) -> Result<u64> {
    if text == "0" { return Ok(0); }
    if !text.starts_with("0x") { return Err("KERNEL_EXEC_NOT_RAW"); }
    kernel_u64(text).map_err(|_| "KERNEL_EXEC_NOT_RAW")
}
fn kernel_raw_exec(call: KernelCall, fields: &KernelFields<'_>) -> Result<Option<i32>> {
    fields.arity(if call == KernelCall::Execve { 3 } else { 5 })?;
    let offset = usize::from(call == KernelCall::Execveat);
    if kernel_raw_word(fields.get(offset)?)? == 0 || kernel_raw_word(fields.get(offset + 1)?)? == 0 { return Err("KERNEL_EXEC_RAW_POINTER"); }
    kernel_raw_word(fields.get(offset + 2)?)?;
    if call == KernelCall::Execve { return Ok(None); }
    let raw_fd = kernel_raw_word(fields.get(0)?)?;
    let flags = kernel_raw_word(fields.get(4)?)?;
    if flags & !(0x100 | 0x1000) != 0 { return Err("KERNEL_EXEC_RAW_FLAGS"); }
    if matches!(raw_fd, 0xffffff9c | 0xffffffffffffff9c) { return Ok(None); }
    if raw_fd > i32::MAX as u64 { return Err("KERNEL_EXEC_RAW_DIRFD"); }
    Ok(Some(raw_fd as i32))
}
fn kernel_signal_set(text: &str) -> Result<()> {
    let text = text.strip_prefix('~').unwrap_or(text);
    let inner = text.strip_prefix('[').and_then(|text| text.strip_suffix(']')).ok_or("KERNEL_SIGNALSET_GRAMMAR")?;
    let mut seen = 0u64;
    for token in inner.split_whitespace() {
        let signal = match token {
            "HUP" => 1, "INT" => 2, "QUIT" => 3, "ILL" => 4, "TRAP" => 5, "ABRT" => 6,
            "BUS" => 7, "FPE" => 8, "KILL" => 9, "USR1" => 10, "SEGV" => 11, "USR2" => 12,
            "PIPE" => 13, "ALRM" => 14, "TERM" => 15, "STKFLT" => 16, "CHLD" => 17,
            "CONT" => 18, "STOP" => 19, "TSTP" => 20, "TTIN" => 21, "TTOU" => 22,
            "URG" => 23, "XCPU" => 24, "XFSZ" => 25, "VTALRM" => 26, "PROF" => 27,
            "WINCH" => 28, "IO" => 29, "PWR" => 30, "SYS" => 31,
            value if value.starts_with("RT_") => {
                let offset = kernel_u32(&value[3..])?;
                if !(1..=32).contains(&offset) { return Err("KERNEL_SIGNALSET_GRAMMAR"); }
                32 + offset
            },
            _ => return Err("KERNEL_SIGNALSET_GRAMMAR"),
        };
        let bit = 1u64 << (signal - 1);
        if seen & bit != 0 { return Err("KERNEL_SIGNALSET_GRAMMAR"); } seen |= bit;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KernelErrno { None, InProgress, Interrupted, Restarted, Other }
struct KernelResult<'a> { annotation: Option<&'a str>, errno: KernelErrno, restarted: bool, flags_suffix: u8 }
fn kernel_errno(text: &str) -> Result<KernelErrno> {
    Ok(match text {
        "EINPROGRESS" => KernelErrno::InProgress, "EINTR" => KernelErrno::Interrupted,
        "ERESTARTSYS" | "ERESTARTNOINTR" | "ERESTARTNOHAND" | "ERESTART_RESTARTBLOCK" => KernelErrno::Restarted,
        "EPERM" | "ENOENT" | "ESRCH" | "EIO" | "ENXIO" | "E2BIG" | "ENOEXEC" | "EBADF" |
        "ECHILD" | "EAGAIN" | "EWOULDBLOCK" | "ENOMEM" | "EACCES" | "EFAULT" | "ENOTBLK" |
        "EBUSY" | "EEXIST" | "EXDEV" | "ENODEV" | "ENOTDIR" | "EISDIR" | "EINVAL" |
        "ENFILE" | "EMFILE" | "ENOTTY" | "ETXTBSY" | "EFBIG" | "ENOSPC" | "ESPIPE" |
        "EROFS" | "EMLINK" | "EPIPE" | "EDOM" | "ERANGE" | "EDEADLK" | "ENAMETOOLONG" |
        "ENOLCK" | "ENOSYS" | "ENOTEMPTY" | "ELOOP" | "ENOMSG" | "EIDRM" | "ECHRNG" |
        "EL2NSYNC" | "EL3HLT" | "EL3RST" | "ELNRNG" | "EUNATCH" | "ENOCSI" | "EL2HLT" |
        "EBADE" | "EBADR" | "EXFULL" | "ENOANO" | "EBADRQC" | "EBADSLT" | "EBFONT" |
        "ENOSTR" | "ENODATA" | "ETIME" | "ENOSR" | "ENONET" | "ENOPKG" | "EREMOTE" |
        "ENOLINK" | "EADV" | "ESRMNT" | "ECOMM" | "EPROTO" | "EMULTIHOP" | "EDOTDOT" |
        "EBADMSG" | "EOVERFLOW" | "ENOTUNIQ" | "EBADFD" | "EREMCHG" | "ELIBACC" |
        "ELIBBAD" | "ELIBSCN" | "ELIBMAX" | "ELIBEXEC" | "EILSEQ" | "ERESTART" |
        "ESTRPIPE" | "EUSERS" | "ENOTSOCK" | "EDESTADDRREQ" | "EMSGSIZE" | "EPROTOTYPE" |
        "ENOPROTOOPT" | "EPROTONOSUPPORT" | "ESOCKTNOSUPPORT" | "EOPNOTSUPP" | "ENOTSUP" |
        "EPFNOSUPPORT" | "EAFNOSUPPORT" | "EADDRINUSE" | "EADDRNOTAVAIL" | "ENETDOWN" |
        "ENETUNREACH" | "ENETRESET" | "ECONNABORTED" | "ECONNRESET" | "ENOBUFS" |
        "EISCONN" | "ENOTCONN" | "ESHUTDOWN" | "ETOOMANYREFS" | "ETIMEDOUT" |
        "ECONNREFUSED" | "EHOSTDOWN" | "EHOSTUNREACH" | "EALREADY" | "ESTALE" |
        "EUCLEAN" | "ENOTNAM" | "ENAVAIL" | "EISNAM" | "EREMOTEIO" | "EDQUOT" |
        "ENOMEDIUM" | "EMEDIUMTYPE" | "ECANCELED" | "ENOKEY" | "EKEYEXPIRED" |
        "EKEYREVOKED" | "EKEYREJECTED" | "EOWNERDEAD" | "ENOTRECOVERABLE" | "ERFKILL" |
        "EHWPOISON" => KernelErrno::Other,
        _ => return Err("KERNEL_ERRNO_UNSUPPORTED"),
    })
}
fn kernel_result(call: KernelCall, returned: i64, text: &str) -> Result<KernelResult<'_>> {
    let text = text.trim();
    let mut result = KernelResult { annotation: None, errno: KernelErrno::None, restarted: false, flags_suffix: 0 };
    let (number, rest) = if let Some(rest) = text.strip_prefix("? ") {
        if returned != -1 { return Err("KERNEL_RESTART_RESULT"); }
        result.restarted = true; ("?", rest)
    } else {
        let end = text.find(|byte: char| byte == '<' || byte.is_ascii_whitespace()).unwrap_or(text.len());
        (&text[..end], &text[end..])
    };
    if number != "?" {
        let actual = if number == "-1" { -1 } else { i64::try_from(kernel_u64(number)?).map_err(|_| "KERNEL_SYSCALL_RESULT")? };
        if actual != returned { return Err("KERNEL_SYSCALL_RESULT_CONFLICT"); }
    }
    if returned == -1 {
        let rest = rest.trim_start();
        let (errno, description) = rest.split_once(' ').unwrap_or((rest, ""));
        result.errno = kernel_errno(errno)?;
        if !description.is_empty() && (description.len() > 512 || !description.starts_with('(') || !description.ends_with(')') ||
            !description.bytes().all(|byte| byte == b' ' || byte.is_ascii_graphic())) { return Err("KERNEL_ERRNO_GRAMMAR"); }
        if result.restarted && result.errno != KernelErrno::Restarted { return Err("KERNEL_RESTART_RESULT"); }
        result.restarted |= result.errno == KernelErrno::Restarted;
        return Ok(result);
    }
    if returned < 0 { return Err("KERNEL_SYSCALL_RESULT"); }
    if let Some(annotation) = rest.strip_prefix('<') {
        let annotation = annotation.strip_suffix('>').ok_or("KERNEL_RESULT_ANN_WRAP")?;
        // The same device-class tag strace appends to argument annotations also
        // appears on returned descriptors, e.g. "= 6</dev/null<char 1:3>>". Strip
        // exactly that documented suffix here too; the tag is not a socket target.
        kernel_annotation(strip_device_tag(annotation).unwrap_or(annotation))?;
        let fd_result = matches!(call, KernelCall::Socket | KernelCall::Accept | KernelCall::Accept4 |
            KernelCall::Dup | KernelCall::Dup2 | KernelCall::Dup3 | KernelCall::Fcntl |
            KernelCall::Open | KernelCall::Openat | KernelCall::Openat2 | KernelCall::Creat |
            KernelCall::Eventfd | KernelCall::Eventfd2 | KernelCall::EpollCreate | KernelCall::EpollCreate1 |
            KernelCall::Signalfd | KernelCall::Signalfd4 | KernelCall::TimerfdCreate);
        if !fd_result { return Err("KERNEL_UNEXPECTED_RESULT_ANNOTATION"); }
        result.annotation = Some(annotation);
    } else if !rest.trim().is_empty() {
        if call != KernelCall::Fcntl { return Err("KERNEL_RESULT_SUFFIX"); }
        let flags = rest.trim().strip_prefix("(flags ").and_then(|text| text.strip_suffix(')')).ok_or("KERNEL_RESULT_SUFFIX")?;
        if flags == "FD_CLOEXEC" {
            if returned != 1 { return Err("KERNEL_FCNTL_FD_FLAGS"); }
            result.flags_suffix = 1;
        } else { kernel_open_flags(flags)?; result.flags_suffix = 2; }
    }
    Ok(result)
}
struct KernelAnnotation {
    socket: bool,
    // 0 opaque socket, 1 IPv4 TCP, 2 IPv6 TCP, 3 other socket.
    protocol: u8,
    complete: bool,
    connected: bool,
    listener_local: bool,
    backend_peer: bool,
}
fn kernel_annotation(text: &str) -> Result<KernelAnnotation> {
    if text.is_empty() { return Err("KERNEL_ANN_EMPTY"); }
    if text.len() > KERNEL_FD_TEXT_LIMIT { return Err("KERNEL_ANN_TOO_LONG"); }
    if text.contains('<') { return Err("KERNEL_ANN_NESTED"); }
    if text.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n')) { return Err("KERNEL_ANN_CONTROL"); }
    let mut parsed = KernelAnnotation { socket: false, protocol: 0, complete: false, connected: false,
        listener_local: false, backend_peer: false };
    let Some((prefix, body)) = text.split_once(": [").or_else(|| text.split_once(":[")) else { return Ok(parsed); };
    let protocol = match prefix {
        "socket" => 0, "TCP" => 1, "TCPv6" => 2,
        "UDP" | "UDPv6" | "UDPLITE" | "UDPLITEv6" | "UNIX" | "UNIX-STREAM" | "UNIX-DGRAM" |
        "UNIX-SEQPACKET" | "NETLINK" | "PACKET" | "ICMP" | "ICMPv6" | "RAW" | "RAWv6" => 3,
        _ => return Ok(parsed),
    };
    parsed.socket = true; parsed.protocol = protocol;
    let body = body.strip_suffix(']').ok_or("KERNEL_SOCKET_ANNOTATION")?;
    if protocol != 1 && protocol != 2 { return Ok(parsed); }
    if body.is_empty() { return Err("KERNEL_SOCKET_ANNOTATION"); }
    if body.bytes().all(|byte| byte.is_ascii_digit()) { kernel_u64(body)?; return Ok(parsed); }
    let (local, peer) = match body.split_once("->") {
        Some((local, peer)) => (local, Some(peer)), None => (body, None),
    };
    let local = local.parse::<std::net::SocketAddr>().map_err(|_| "KERNEL_SOCKET_ANNOTATION")?;
    if (protocol == 1 && !local.is_ipv4()) || (protocol == 2 && !local.is_ipv6()) { return Err("KERNEL_SOCKET_ANNOTATION"); }
    parsed.complete = true;
    parsed.listener_local = local == std::net::SocketAddr::from(([127, 0, 0, 1], 18634));
    if let Some(peer) = peer {
        let peer = peer.parse::<std::net::SocketAddr>().map_err(|_| "KERNEL_SOCKET_ANNOTATION")?;
        if (protocol == 1 && !peer.is_ipv4()) || (protocol == 2 && !peer.is_ipv6()) || peer.port() == 0 { return Err("KERNEL_SOCKET_ANNOTATION"); }
        parsed.connected = true;
        parsed.backend_peer = peer == std::net::SocketAddr::from(([127, 0, 0, 1], 18631));
    }
    Ok(parsed)
}

// Controlled pre-exec child; no socket can be created until the owned stdin gate is released.
use std::os::fd::AsRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};

const GATED_STATUS_BYTES: usize = 16 * 1024;
const GATED_STAT_BYTES: usize = 4096;
const GATED_FD_LIMIT: usize = 16;
const GATED_SCRIPT: &str =
    "IFS= read -r gate && [ \"$gate\" = FP_START ] || exit 70; exec </dev/null; exec \"$@\"";
const GATED_RELEASE: &[u8] = b"FP_START\n";

struct GatedFrontend {
    pid: u32,
    child: Child,
    gate: Option<std::process::ChildStdin>,
    initial_fds: Vec<i32>,
    session: ProcessSession,
    proof: Value,
}

impl GatedFrontend {
    // Main calls this only after proving the sole expected tracer attachment.
    // One small, atomic pipe write is attempted; an error/short write is not retried.
    fn release(&mut self) -> Result<()> {
        let mut gate = self.gate.take().ok_or("GATED_GATE_NOT_AVAILABLE")?;
        self.proof["gate_release_attempted"] = json!(true);
        let written = gate.write(GATED_RELEASE);
        drop(gate);
        match written {
            Ok(count) if count == GATED_RELEASE.len() => {
                self.proof["gate_state_id"] = json!("RELEASED");
                self.proof["gate_release_succeeded"] = json!(true);
                Ok(())
            }
            _ => {
                self.proof["gate_state_id"] = json!("CLOSED_RELEASE_FAILED");
                Err("GATED_RELEASE_WRITE")
            }
        }
    }

    fn rollback_before_release(&mut self) -> Result<()> {
        let gate = self.gate.take().ok_or("GATED_GATE_NOT_AVAILABLE")?;
        drop(gate);
        self.proof["gate_state_id"] = json!("CLOSED_BEFORE_RELEASE");
        self.proof["rollback_closed_before_release"] = json!(true);
        Ok(())
    }

    fn projection(&self) -> Value { self.proof.clone() }
}

// There is deliberately no terminating Drop guard. Dropping an unreleased gate
// supplies EOF to the fixed shell script; dropping std::process::Child neither
// signals nor kills the independently owned launcher or a released frontend.
#[derive(Clone, Copy, PartialEq, Eq)]
struct GatedFileIdentity { device: u64, inode: u64, kind: u32, special: u64 }

impl GatedFileIdentity {
    fn metadata(value: &fs::Metadata) -> Self {
        Self { device: value.dev(), inode: value.ino(),
            kind: value.mode() & libc::S_IFMT as u32, special: value.rdev() }
    }

    fn pipe(gate: &std::process::ChildStdin) -> Result<Self> {
        let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(gate.as_raw_fd(), value.as_mut_ptr()) } != 0 {
            return Err("GATED_PIPE_METADATA");
        }
        let value = unsafe { value.assume_init() };
        let identity = Self { device: value.st_dev as u64, inode: value.st_ino as u64,
            kind: value.st_mode as u32 & libc::S_IFMT as u32, special: value.st_rdev as u64 };
        if identity.kind != libc::S_IFIFO as u32 { return Err("GATED_PIPE_TYPE"); }
        let flags = unsafe { libc::fcntl(gate.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 || flags & libc::O_ACCMODE != libc::O_WRONLY || flags & libc::O_NONBLOCK != 0 {
            return Err("GATED_PIPE_FLAGS");
        }
        Ok(identity)
    }
}

fn gated_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline { Err("GATED_START_DEADLINE") } else { Ok(()) }
}

fn gated_pause(deadline: Instant) -> Result<()> {
    let remaining = deadline.checked_duration_since(Instant::now()).ok_or("GATED_START_DEADLINE")?;
    if remaining.is_zero() { return Err("GATED_START_DEADLINE"); }
    thread::sleep(remaining.min(Duration::from_millis(20)));
    Ok(())
}

// The limit includes every retained byte; a separate one-byte probe detects
// overflow without unbounded allocation. EINTR and other read errors fail closed.
fn gated_read<const N: usize>(path: &Path, deadline: Instant) -> Result<([u8; N], usize)> {
    gated_deadline(deadline)?;
    let mut file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW)
        .open(path).map_err(|_| "GATED_PROC_OPEN")?;
    let mut bytes = [0u8; N];
    let mut used = 0;
    while used < N {
        gated_deadline(deadline)?;
        let count = file.read(&mut bytes[used..]).map_err(|_| "GATED_PROC_READ")?;
        gated_deadline(deadline)?;
        if count == 0 { return Ok((bytes, used)); }
        used += count;
    }
    let mut extra = [0u8; 1];
    gated_deadline(deadline)?;
    let count = file.read(&mut extra).map_err(|_| "GATED_PROC_READ")?;
    gated_deadline(deadline)?;
    if count != 0 { return Err("GATED_PROC_BOUND"); }
    Ok((bytes, used))
}

fn gated_u64(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
    value.bytes().try_fold(0u64, |number, byte|
        number.checked_mul(10)?.checked_add(u64::from(byte - b'0')))
}

fn gated_u32(value: &str) -> Option<u32> {
    u32::try_from(gated_u64(value)?).ok()
}

fn gated_pid(value: &str) -> Option<u32> {
    gated_u32(value).filter(|pid| *pid > 0 && *pid <= i32::MAX as u32)
}

fn gated_fields<const N: usize>(text: &str, key: &str) -> Result<[u32; N]> {
    let mut lines = text.lines().filter_map(|line| line.strip_prefix(key));
    let mut fields = lines.next().ok_or("GATED_STATUS_FIELD")?.split_ascii_whitespace();
    if lines.next().is_some() { return Err("GATED_STATUS_FIELD"); }
    let mut values = [0; N];
    for value in &mut values {
        *value = fields.next().and_then(gated_u32).ok_or("GATED_STATUS_FIELD")?;
    }
    if fields.next().is_some() { return Err("GATED_STATUS_FIELD"); }
    Ok(values)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GatedStatus {
    pid: u32, tgid: u32, ppid: u32, uid: [u32; 4], gid: [u32; 4], threads: u32, tracer: u32, supplementary_groups:u32,
}

fn gated_status(path: &Path, deadline: Instant) -> Result<GatedStatus> {
    let (bytes, used) = gated_read::<GATED_STATUS_BYTES>(path, deadline)?;
    let text = std::str::from_utf8(&bytes[..used]).map_err(|_| "GATED_STATUS_ENCODING")?;
    if !text.ends_with('\n') { return Err("GATED_STATUS_INCOMPLETE"); }
    gated_fields::<0>(text,"Groups:")?;
    Ok(GatedStatus {
        pid: gated_fields::<1>(text, "Pid:")?[0],
        tgid: gated_fields::<1>(text, "Tgid:")?[0],
        ppid: gated_fields::<1>(text, "PPid:")?[0],
        uid: gated_fields::<4>(text, "Uid:")?,
        gid: gated_fields::<4>(text, "Gid:")?,
        threads: gated_fields::<1>(text, "Threads:")?[0],
        tracer: gated_fields::<1>(text, "TracerPid:")?[0], supplementary_groups:0,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GatedProcStamp { ppid: u32, pgid: u32, sid: u32, threads: u32, start: u64 }

fn gated_stamp(pid: u32, deadline: Instant) -> Result<GatedProcStamp> {
    let (bytes, used) = gated_read::<GATED_STAT_BYTES>(Path::new(&format!("/proc/{pid}/stat")), deadline)?;
    let text = std::str::from_utf8(&bytes[..used]).map_err(|_| "GATED_STAT_ENCODING")?;
    if !text.ends_with('\n') { return Err("GATED_STAT_INCOMPLETE"); }
    let (number, tail) = text.split_once(" (").ok_or("GATED_STAT_FIELD")?;
    if gated_pid(number) != Some(pid) { return Err("GATED_STAT_IDENTITY"); }
    let (_, fields) = tail.rsplit_once(") ").ok_or("GATED_STAT_FIELD")?;
    let mut fields = fields.split_ascii_whitespace();
    let mut stamp = GatedProcStamp { ppid: 0, pgid: 0, sid: 0, threads: 0, start: 0 };
    for index in 0..20 {
        let field = fields.next().ok_or("GATED_STAT_FIELD")?;
        match index {
            0 if !matches!(field, "R" | "S" | "D" | "I") => return Err("GATED_PROCESS_STATE"),
            1 => stamp.ppid = gated_pid(field).ok_or("GATED_STAT_FIELD")?,
            2 => stamp.pgid = gated_pid(field).ok_or("GATED_STAT_FIELD")?,
            3 => stamp.sid = gated_pid(field).ok_or("GATED_STAT_FIELD")?,
            17 => stamp.threads = gated_u32(field).ok_or("GATED_STAT_FIELD")?,
            19 => stamp.start = gated_u64(field).ok_or("GATED_STAT_FIELD")?,
            _ => {},
        }
    }
    Ok(stamp)
}

fn gated_single_thread(pid: u32, expected: GatedStatus, deadline: Instant) -> Result<()> {
    gated_deadline(deadline)?;
    let entries = fs::read_dir(format!("/proc/{pid}/task")).map_err(|_| "GATED_TASK_OPEN")?;
    let mut seen = false;
    for entry in entries {
        gated_deadline(deadline)?;
        if seen { return Err("GATED_TASK_NOT_SINGLE"); }
        let entry = entry.map_err(|_| "GATED_TASK_READ")?;
        let name = entry.file_name();
        let tid = name.to_str().and_then(gated_pid).ok_or("GATED_TASK_NUMERIC")?;
        if tid != pid || gated_status(&entry.path().join("status"), deadline)? != expected {
            return Err("GATED_TASK_IDENTITY");
        }
        seen = true;
    }
    if !seen { return Err("GATED_TASK_NOT_SINGLE"); }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GatedFdClass { GateRead, OwnedLog, DevNull }

impl GatedFdClass {
    fn id(self) -> &'static str {
        match self { Self::GateRead => "CONTROLLED_GATE_READ", Self::OwnedLog => "ROOT_PREOPENED_LOG",
            Self::DevNull => "DEVNULL" }
    }
}

fn gated_inventory(pid: u32, gate: GatedFileIdentity, log: GatedFileIdentity,
    null: GatedFileIdentity, deadline: Instant) -> Result<Vec<(i32, GatedFdClass)>> {
    gated_deadline(deadline)?;
    let entries = fs::read_dir(format!("/proc/{pid}/fd")).map_err(|_| "GATED_FD_OPEN")?;
    let mut inventory = Vec::with_capacity(GATED_FD_LIMIT);
    for entry in entries {
        gated_deadline(deadline)?;
        if inventory.len() >= GATED_FD_LIMIT { return Err("GATED_FD_BOUND"); }
        let entry = entry.map_err(|_| "GATED_FD_READ")?;
        let name = entry.file_name();
        if !name.as_bytes().iter().all(|byte| byte.is_ascii_digit()) { return Err("GATED_FD_NUMERIC"); }
        let fd = name.to_str().and_then(gated_u32).and_then(|value| i32::try_from(value).ok())
            .ok_or("GATED_FD_NUMERIC")?;
        let path = entry.path();
        let before = fs::metadata(&path).map_err(|_| "GATED_FD_METADATA")?;
        let identity = GatedFileIdentity::metadata(&before);
        let after = fs::metadata(&path).map_err(|_| "GATED_FD_METADATA")?;
        if GatedFileIdentity::metadata(&after) != identity { return Err("GATED_FD_CHANGED"); }
        let class = if fd == 0 {
            if identity != gate {
                return Err("GATED_STDIN_IDENTITY");
            }
            GatedFdClass::GateRead
        } else if fd == 1 || fd == 2 || identity == log {
            if identity != log { return Err("GATED_LOG_IDENTITY"); }
            GatedFdClass::OwnedLog
        } else if identity == null {
            GatedFdClass::DevNull
        } else {
            return Err("GATED_FD_UNKNOWN_OR_SOCKET");
        };
        inventory.push((fd, class));
    }
    inventory.sort_unstable_by_key(|entry| entry.0);
    if inventory.len() != 3 || inventory[0].0 != 0 || inventory[1].0 != 1 || inventory[2].0 != 2 ||
        inventory.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("GATED_STDIO_INCOMPLETE");
    }
    gated_deadline(deadline)?;
    Ok(inventory)
}

// Following the fixed canonical executable pathname yields its inode/device
// identity without retaining or exporting a proc symlink target or pathname.
fn gated_executable(path: &Path) -> Result<GatedFileIdentity> {
    let metadata = fs::metadata(path).map_err(|_| "GATED_EXECUTABLE_METADATA")?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o111 == 0 || metadata.mode() & 0o022 != 0 {
        return Err("GATED_EXECUTABLE_IDENTITY");
    }
    Ok(GatedFileIdentity::metadata(&metadata))
}

fn prepare_gated_frontend(work:&Path,observer:ProcessSession,old_pid:u32,deadline:Instant)->Result<GatedFrontend> {
    use std::os::unix::process::CommandExt;
    gated_deadline(deadline)?;
    let parent_credentials=unsafe {(libc::getuid(),libc::geteuid(),libc::getgid(),libc::getegid())};
    if old_pid==0 || old_pid>i32::MAX as u32 || observer.pid!=std::process::id() || observer.pid!=observer.sid ||
        observer.pid!=observer.pgid || ProcessSession::read(observer.pid)?!=observer || parent_credentials.0!=0 || parent_credentials.1!=0 {return Err("GATED_OBSERVER_IDENTITY");}
    let shell=gated_executable(Path::new("/bin/sh"))?;
    let setsid=gated_executable(Path::new("/usr/bin/setsid"))?;
    let self_exe=GatedFileIdentity::metadata(&fs::metadata("/proc/self/exe").map_err(|_|"GATED_SELF_EXE")?);
    let null_metadata=fs::symlink_metadata("/dev/null").map_err(|_|"GATED_DEVNULL_METADATA")?;
    if !null_metadata.file_type().is_char_device(){return Err("GATED_DEVNULL_IDENTITY");}
    let null=GatedFileIdentity::metadata(&null_metadata);
    let prior_log=fs::symlink_metadata(work.join("frontend.log")).map_err(|_|"GATED_LOG_METADATA")?;
    if !prior_log.is_file() || prior_log.uid()!=0 {return Err("GATED_LOG_OWNERSHIP");}
    let log=fs::OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW).open(work.join("frontend.log")).map_err(|_|"GATED_LOG_OPEN")?;
    let metadata=log.metadata().map_err(|_|"GATED_LOG_METADATA")?;
    if !metadata.is_file() || metadata.uid()!=0 || metadata.nlink()!=1 || GatedFileIdentity::metadata(&metadata)!=GatedFileIdentity::metadata(&prior_log){return Err("GATED_LOG_OWNERSHIP");}
    let log_identity=GatedFileIdentity::metadata(&metadata);
    log.set_len(0).map_err(|_|"GATED_LOG_TRUNCATE")?;
    let stderr=log.try_clone().map_err(|_|"GATED_LOG_DUPLICATE")?;
    let mut child=Command::new("/usr/bin/setsid").args(["/bin/sh","-c",GATED_SCRIPT,"fp-owned-start",EXE,
        "--config","/opt/ferriprint-windows-local-qa/ferriprint.toml","serve"])
        .gid(979).uid(994).stdin(Stdio::piped()).stdout(Stdio::from(log)).stderr(Stdio::from(stderr))
        .spawn().map_err(|error|{
            if let Ok(mut value)=load_restart(work){value["startup_spawn_errno"]=json!(error.raw_os_error());value["preservation_required"]=json!(true);let _=save_restart(work,&value);}
            "GATED_LAUNCH_SPAWN"
        })?;
    if unsafe {(libc::getuid(),libc::geteuid(),libc::getgid(),libc::getegid())}!=parent_credentials {return Err("GATED_PARENT_CREDENTIALS_CHANGED");}
    let gate=child.stdin.take().ok_or("GATED_PIPE_MISSING")?;let gate_identity=GatedFileIdentity::pipe(&gate)?;
    let pid=child.id();if pid==old_pid || pid==observer.pid || pid>i32::MAX as u32{return Err("GATED_CHILD_PID");}
    let start=gated_stamp(pid,deadline)?.start;
    loop {
        gated_deadline(deadline)?;
        if child.try_wait().map_err(|_|"GATED_CHILD_WAIT")?.is_some(){return Err("GATED_CHILD_EXITED");}
        let stamp=gated_stamp(pid,deadline)?;let status=gated_status(Path::new(&format!("/proc/{pid}/status")),deadline)?;
        if stamp.start!=start || stamp.ppid!=observer.pid || stamp.threads!=1 || status.pid!=pid || status.tgid!=pid ||
            status.ppid!=observer.pid || status.threads!=1 || status.tracer!=0 {return Err("GATED_CHILD_IDENTITY");}
        let image=GatedFileIdentity::metadata(&fs::metadata(format!("/proc/{pid}/exe")).map_err(|_|"GATED_CHILD_EXECUTABLE")?);
        if image!=shell {
            if image!=setsid && image!=self_exe{return Err("GATED_CHILD_EXECUTABLE");}
            gated_pause(deadline)?;continue;
        }
        if status.uid!=[994;4] || status.gid!=[979;4] || status.supplementary_groups!=0 {return Err("GATED_CHILD_CREDENTIALS");}
        let session=ProcessSession::read(pid)?;
        if session.pid!=session.sid || session.pid!=session.pgid || stamp.sid!=session.sid || stamp.pgid!=session.pgid ||
            !observer.isolated_from(session){return Err("GATED_SESSION_IDENTITY");}
        gated_single_thread(pid,status,deadline)?;
        let inventory=gated_inventory(pid,gate_identity,log_identity,null,deadline)?;
        if gated_stamp(pid,deadline)?!=stamp || gated_status(Path::new(&format!("/proc/{pid}/status")),deadline)?!=status ||
            ProcessSession::read(pid)?!=session || GatedFileIdentity::metadata(&fs::metadata(format!("/proc/{pid}/exe")).map_err(|_|"GATED_CHILD_EXECUTABLE")?)!=shell ||
            child.try_wait().map_err(|_|"GATED_CHILD_WAIT")?.is_some(){return Err("GATED_PREEXEC_CHANGED");}
        let proof=json!({"method_id":"COMMAND_EXT_UID_GID_CHILD_ONLY_STDIN_GATE","child_pid":pid,"ppid":observer.pid,
            "observer":observer.projection(),"target_session_before_exec":session.projection(),"uid":994,"gid":979,
            "supplementary_groups_empty":true,"parent_credentials_unchanged":true,"canonical_sh_identity_verified":true,
            "direct_Child_id_verified":true,"thread_count":1,"tracer_pid_before_attach":0,"different_from_old_pid":true,
            "root_preopened_log_identity_verified":true,"root_preopened_stdout_stderr_alias":true,
            "controlled_gate_pipe_identity_verified":true,"initial_fd_limit":3,"initial_fd_count":inventory.len(),
            "initial_fds":inventory.iter().map(|(fd,class)|json!({"fd":fd,"class_id":class.id()})).collect::<Vec<_>>(),
            "no_initial_socket_or_unknown_fds":true,"stdio_constructor_non_CLOEXEC_inheritance":true,
            "fixed_script_redirects_stdin_to_devnull":true,"gate_state_id":"HELD","gate_release_attempted":false,
            "gate_release_succeeded":false,"rollback_closed_before_release":false,"child_owned_without_terminating_drop":true});
        return Ok(GatedFrontend{pid,child,gate:Some(gate),initial_fds:inventory.iter().map(|v|v.0).collect(),session,proof});
    }
}

    fn restart_ledger_readonly_for_service_uid(context:&mut LedgerFailureContext)->Result<Value> {
        context.credentials();
        if context.getgroups_count!=Some(0)||context.uid!=Some(994)||context.euid!=Some(994)||context.gid!=Some(979)||context.egid!=Some(979){return Err("LEDGER_SERVICE_CREDENTIALS");}
        use rusqlite::{Connection,OpenFlags,OptionalExtension,TransactionBehavior};
        use std::os::unix::fs::MetadataExt;
        let path=Path::new(QA_STATE).join("jobs.sqlite3");context.stage="DB_METADATA";
        let metadata=fs::symlink_metadata(&path).map_err(|error|{context.db_metadata_errno=error.raw_os_error();"RESTART_LEDGER_METADATA"})?;
        context.db_metadata_observed=true;context.db_file_type_bits=Some(metadata.mode()&libc::S_IFMT as u32);
        context.db_is_file=Some(metadata.is_file());context.db_is_symlink=Some(metadata.file_type().is_symlink());
        context.db_uid=Some(metadata.uid());context.db_gid=Some(metadata.gid());context.db_mode=Some(metadata.mode()&0o7777);
        context.stage="DB_IDENTITY";
        if !metadata.is_file()||metadata.file_type().is_symlink()||metadata.uid()!=994||metadata.gid()!=979||metadata.mode()&0o7777!=0o600{return Err("RESTART_LEDGER_IDENTITY");}
        // SQL read-only sees committed WAL; any SQLite sidecar is service-owned.
        // No immutable1, JobEngine acquisition, migration, DB copy or permission repair.
        context.stage="DB_OPEN";
        let mut db=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX|OpenFlags::SQLITE_OPEN_NOFOLLOW).map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_READONLY_OPEN"))?;
        context.stage="DB_TRANSACTION";
        let tx=db.transaction_with_behavior(TransactionBehavior::Deferred).map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_READONLY_SNAPSHOT"))?;
        context.stage="JOB_COUNT";
        let count:i64=tx.query_row("SELECT COUNT(*) FROM jobs",[],|r|r.get(0)).map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_COUNT"))?;
        context.stage="ACTIVE_JOB_COUNT";
        let active:i64=tx.query_row("SELECT COUNT(*) FROM jobs WHERE state < 7",[],|r|r.get(0)).map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_COUNT"))?;
        context.stage="MAX_JOB_ID";
        let max_id:Option<i64>=tx.query_row("SELECT MAX(id) FROM jobs",[],|r|r.get(0)).map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_IDS"))?;
        context.stage="SQLITE_SEQUENCE";
        let sequence:Option<i64>=tx.query_row("SELECT seq FROM sqlite_sequence WHERE name='jobs'",[],|r|r.get(0)).optional().map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_SEQUENCE"))?;
        context.stage="TIME_ORIGIN";
        let origin:i64=tx.query_row("SELECT value FROM service_meta WHERE key='time-origin'",[],|r|r.get(0)).map_err(|error|context.sqlite_failure(error,"RESTART_LEDGER_TIME_ORIGIN"))?;
        context.stage="ZERO_JOB_GUARD";
        if count!=0||active!=0||max_id.is_some()||sequence.is_some_and(|v|v<0){return Err("RESTART_LEDGER_NOT_ZERO");}
        Ok(json!({"read_only_normal_WAL_snapshot":true,"SQL_readonly_not_filesystem_immutability":true,
            "job_row_count":count,"active_row_count":active,"maximum_job_id":max_id,"sqlite_sequence_jobs":sequence,
            "time_origin":origin,"logical_job_ids":[],"logical_UUID_rows_empty":true,
            "record_owner_title_document_fields_read":false,"DB_copied_or_migrated_or_byte_identity_claim":false,
            "uid":metadata.uid(),"gid":metadata.gid(),"mode":metadata.mode()&0o7777}))
    }


    fn restart_ledger_stdio()->Result<()> {
        let mut context=LedgerFailureContext::default();context.stage="INTERNAL";
        let snapshot=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||restart_ledger_readonly_for_service_uid(&mut context)));
        let value=match snapshot {
            Ok(Ok(value))=>value,
            Ok(Err(id))=>{ledger_emit_failure(&context,id)?;return Err(id);},
            Err(_)=>{ledger_emit_failure(&context,"LEDGER_SERVICE_PANIC")?;return Err("LEDGER_SERVICE_PANIC");},
        };
        context.stage="SERIALIZE";
        let bytes=match serde_json::to_vec(&value){Ok(bytes)=>bytes,Err(_)=>{ledger_emit_failure(&context,"LEDGER_SERVICE_JSON")?;return Err("LEDGER_SERVICE_JSON");}};
        if bytes.len()>LEDGER_STDOUT_LIMIT{ledger_emit_failure(&context,"LEDGER_SERVICE_JSON_BOUND")?;return Err("LEDGER_SERVICE_JSON_BOUND");}
        let mut stdout=std::io::stdout().lock();
        stdout.write_all(&bytes).and_then(|_|stdout.flush()).map_err(|_|"LEDGER_SERVICE_STDOUT")
    }

    fn verified_frontend_identity(pid:u32)->Result<Value> {
            if fs::read_link(format!("/proc/{pid}/exe")).map_err(|_|"FRONTEND_EXE_IDENTITY")?!=Path::new(EXE){return Err("FRONTEND_EXE_IDENTITY");}
            let status=bounded_file(Path::new(&format!("/proc/{pid}/status")),16*1024)?;
            let status=std::str::from_utf8(&status).map_err(|_|"FRONTEND_STATUS")?;
            for (key,expected) in [("Uid:",994u32),("Gid:",979u32)] {
                let mut count=0;
                for value in status.lines().find_map(|v|v.strip_prefix(key)).ok_or("FRONTEND_CREDENTIALS")?.split_whitespace() {
                    if value.parse::<u32>().ok()!=Some(expected){return Err("FRONTEND_CREDENTIALS");}count+=1;
                }
                if count!=4{return Err("FRONTEND_CREDENTIALS");}
            }
            let mut groups=Vec::new();
            for value in status.lines().find_map(|v|v.strip_prefix("Groups:")).ok_or("FRONTEND_GROUPS")?.split_whitespace() {
                if groups.len()>=32{return Err("FRONTEND_GROUPS_BOUND");}groups.push(value.parse::<u32>().map_err(|_|"FRONTEND_GROUPS")?);
            }
            if number(status,"Tgid:")?!=pid{return Err("FRONTEND_TGID");}
            let hash=Command::new("sha256sum").arg(format!("/proc/{pid}/exe")).stdout(Stdio::piped()).stderr(Stdio::null()).output().map_err(|_|"FRONTEND_ELF_HASH")?;
            if !hash.status.success() || hash.stdout.get(..64)!=Some(ELF.as_bytes()){return Err("FRONTEND_ELF_HASH");}
            let stat=bounded_file(Path::new(&format!("/proc/{pid}/stat")),4096)?;
            let stat=std::str::from_utf8(&stat).map_err(|_|"FRONTEND_START_IDENTITY")?;
            let start=stat.rsplit_once(')').ok_or("FRONTEND_START_IDENTITY")?.1.split_whitespace().nth(19)
                .and_then(|v|v.parse::<u64>().ok()).ok_or("FRONTEND_START_IDENTITY")?;
            Ok(json!({"pid":pid,"uid":994,"gid":979,"tgid":pid,"ppid":number(status,"PPid:")?,"supplementary_groups":groups,"start_time_ticks":start,"elf_sha256":ELF,
                "session":ProcessSession::read(pid)?.projection()}))
        }
    const QA_STATE:&str="/var/lib/ferriprint-windows-local-qa";
    fn restart_file_fingerprint(path:&Path,uid:u32,mode:u32)->Result<Value> {
        use std::os::unix::fs::MetadataExt;
        let file=fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path).map_err(|_|"RESTART_AUTHORITY_OPEN")?;
        let metadata=file.metadata().map_err(|_|"RESTART_AUTHORITY_METADATA")?;
        if !metadata.is_file() || metadata.uid()!=uid || metadata.gid()!=979 || metadata.mode()&0o7777!=mode || metadata.len()>1024*1024 {return Err("RESTART_AUTHORITY_IDENTITY");}
        let hash=Command::new("sha256sum").stdin(Stdio::from(file)).stdout(Stdio::piped()).stderr(Stdio::null()).output().map_err(|_|"RESTART_AUTHORITY_HASH")?;
        let digest=hash.stdout.get(..64).filter(|v|v.iter().all(|v|v.is_ascii_hexdigit())).ok_or("RESTART_AUTHORITY_HASH")?;
        if !hash.status.success(){return Err("RESTART_AUTHORITY_HASH");}
        Ok(json!({"uid":metadata.uid(),"gid":metadata.gid(),"mode":metadata.mode()&0o7777,"bytes":metadata.len(),
            "sha256":std::str::from_utf8(digest).map_err(|_|"RESTART_AUTHORITY_HASH")?}))
    }
    fn restart_directory(path:&Path)->Result<Value> {
        use std::os::unix::fs::MetadataExt;
        let metadata=fs::symlink_metadata(path).map_err(|_|"RESTART_DIRECTORY_METADATA")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink(){return Err("RESTART_DIRECTORY_IDENTITY");}
        Ok(json!({"uid":metadata.uid(),"gid":metadata.gid(),"mode":metadata.mode()&0o7777}))
    }
    fn restart_ledger_identity(work:&Path)->Result<Value> {
        // Same fixed service-UID snapshot; no parent credential change or retry.
        use std::os::unix::process::CommandExt;
        let child=match Command::new("/opt/ferriprint-windows-local-qa/receiver").arg("--observe-ledger-snapshot").gid(979).uid(994)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output() {
            Ok(child)=>child,
            Err(error)=>{
                let diagnostic=ledger_child_failure_fields("LEDGER_SERVICE_SPAWN",error.raw_os_error(),None,None)?;
                ledger_record_child_failure(work,diagnostic)?;return Err("LEDGER_SERVICE_SPAWN");
            }
        };
        let within_limit=child.stdout.len()<=LEDGER_STDOUT_LIMIT;
        let parsed=if within_limit&&!child.stdout.is_empty(){serde_json::from_slice::<Value>(&child.stdout).ok()}else{None};
        let checked=(||->Result<()> {
            if !child.status.success()||child.stdout.is_empty()||!within_limit{return Err("LEDGER_SERVICE_SNAPSHOT_FAILED");}
            let value=parsed.as_ref().ok_or("LEDGER_SERVICE_JSON")?;
            let object=value.as_object().ok_or("LEDGER_SERVICE_JSON_SHAPE")?;
            let keys=["read_only_normal_WAL_snapshot","SQL_readonly_not_filesystem_immutability","job_row_count","active_row_count",
                "maximum_job_id","sqlite_sequence_jobs","time_origin","logical_job_ids","logical_UUID_rows_empty",
                "record_owner_title_document_fields_read","DB_copied_or_migrated_or_byte_identity_claim","uid","gid","mode"];
            if object.len()!=keys.len() || !keys.iter().all(|key|object.contains_key(*key)){return Err("LEDGER_SERVICE_JSON_SHAPE");}
            for key in ["job_row_count","active_row_count","time_origin","uid","gid","mode"] {
                if !value[key].is_i64() && !value[key].is_u64(){return Err("LEDGER_SERVICE_JSON_TYPE");}
            }
            if value["job_row_count"]!=0 || value["active_row_count"]!=0 || !value["maximum_job_id"].is_null() ||
                !(value["sqlite_sequence_jobs"].is_null() || value["sqlite_sequence_jobs"].as_i64().is_some_and(|v|v>=0)) ||
                value["logical_job_ids"].as_array().is_none_or(|v|!v.is_empty()) || value["logical_UUID_rows_empty"]!=true ||
                value["read_only_normal_WAL_snapshot"]!=true || value["SQL_readonly_not_filesystem_immutability"]!=true ||
                value["record_owner_title_document_fields_read"]!=false || value["DB_copied_or_migrated_or_byte_identity_claim"]!=false ||
                value["uid"]!=994 || value["gid"]!=979 || value["mode"]!=384 {return Err("LEDGER_SERVICE_IDENTITY");}
    
            Ok(())
        })();
        if let Err(id)=checked {
            let diagnostic=ledger_child_failure_fields(id,None,Some(&child),parsed)?;
            ledger_record_child_failure(work,diagnostic)?;return Err(id);
        }
        parsed.ok_or("LEDGER_SERVICE_JSON")
    }

    fn restart_state_identity()->Result<Value> {
        use std::os::unix::ffi::OsStrExt;
        let root=Path::new("/opt/ferriprint-windows-local-qa");let state=Path::new(QA_STATE);
        let state_directory=restart_directory(state)?;
        let profiles_directory=restart_directory(&state.join("profiles"))?;
        let mut profiles=BTreeMap::new();
        for (index,entry) in fs::read_dir(state.join("profiles")).map_err(|_|"RESTART_PROFILES_READ")?.enumerate() {
            if index>=128 {return Err("RESTART_PROFILES_BOUND");}
            let entry=entry.map_err(|_|"RESTART_PROFILES_READ")?;
            if !entry.file_type().map_err(|_|"RESTART_PROFILE_TYPE")?.is_file(){return Err("RESTART_PROFILE_TYPE");}
            let key=uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID,entry.file_name().as_bytes()).to_string();
            if profiles.insert(key,restart_file_fingerprint(&entry.path(),994,0o600)?).is_some(){return Err("RESTART_PROFILE_KEY_COLLISION");}
        }
        if profiles.is_empty(){return Err("RESTART_PROFILE_MISSING");}
        Ok(json!({"state_directory":state_directory,"profiles_directory":profiles_directory,
            "access":restart_file_fingerprint(&state.join("access.toml"),994,0o600)?,
            "catalog":restart_file_fingerprint(&state.join("printers.toml"),994,0o600)?,
            "all_profiles_by_opaque_name_key":profiles,"config":restart_file_fingerprint(&root.join("ferriprint.toml"),0,0o640)?,
            "choices":restart_file_fingerprint(&root.join("choices.toml"),0,0o640)?,
            "public_PPD":restart_file_fingerprint(&root.join("public-native.ppd"),0,0o440)?}))
    }
    fn restart_path(work:&Path)->PathBuf {work.join("proof/native-observer-startup.json")}
    fn save_restart(work:&Path,value:&Value)->Result<()> {
        let tmp=work.join("native-observer-startup.pending");
        let file=fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|_|"RESTART_PROOF_WRITE")?;
        let mut writer=LimitedWriter{inner:std::io::BufWriter::new(file),used:0};
        serde_json::to_writer_pretty(&mut writer,value).map_err(|_|"RESTART_PROOF_LIMIT")?;
        writer.flush().map_err(|_|"RESTART_PROOF_WRITE")?;
        fs::rename(tmp,restart_path(work)).map_err(|_|"RESTART_PROOF_WRITE")
    }
    fn load_restart(work:&Path)->Result<Value> {
        let bytes=bounded_file(&restart_path(work),LIMIT*8)?;
        serde_json::from_slice(&bytes).map_err(|_|"RESTART_PROOF_SHAPE")
    }
    fn quiesce_owned_frontend(work:&Path,pid:u32)->Result<()> {
        if restart_path(work).exists(){return Err("RESTART_ALREADY_ATTEMPTED");}
        let mut proof=json!({"restart_attempted":true,"preservation_required":true,"old_frontend_stopped":false,
            "new_frontend_identity_and_state_proven":false,"release_has_occurred":false,"failure_ids":[]});
        save_restart(work,&proof)?;
        let result=(||->Result<()> {
            let baseline=tasks(pid)?;
            let old_identity=verified_frontend_identity(pid)?;
            if baseline.values().any(|v|*v!=0){return Err("RESTART_OLD_ALREADY_TRACED");}
            let session=ProcessSession::read(pid)?;
            proof["old_frontend_pid"]=json!(pid);proof["old_frontend_session"]=session.projection();
            proof["old_frontend_identity"]=old_identity.clone();save_restart(work,&proof)?;
            let before=restart_state_identity()?;
            proof["before_stop_state"]=before.clone();save_restart(work,&proof)?;
            if verified_frontend_identity(pid)?!=old_identity || tasks(pid)?.values().any(|v|*v!=0){return Err("RESTART_OLD_IDENTITY_CHANGED");}
            if unsafe {libc::kill(pid as i32,libc::SIGTERM)}!=0{return Err("RESTART_OWNED_SIGTERM");}
            proof["owned_old_SIGTERM_sent"]=json!(true);save_restart(work,&proof)?;
            let deadline=Instant::now()+Duration::from_secs(3);
            loop {
                match fs::symlink_metadata(format!("/proc/{pid}")) {
                    Err(error) if error.raw_os_error()==Some(libc::ENOENT)=>break,
                    Err(_)=>return Err("RESTART_OLD_EXIT_UNKNOWN"),Ok(_)=>{}
                }
                if Instant::now()>=deadline{return Err("RESTART_OLD_EXIT_DEADLINE");}
                thread::sleep(Duration::from_millis(20));
            }
            proof["old_frontend_stopped"]=json!(true);
            let after=restart_state_identity()?;
            if after!=before{return Err("RESTART_STATE_CHANGED_DURING_STOP");}
            proof["after_stop_state"]=after;proof["state_preserved_after_old_exit"]=json!(true);
            // The only logical-ledger read is taken here, immediately after the owned
            // frontend has exited: no concurrent product connection then holds the
            // shared-memory DMS byte, so the read involves no competing holder.
            proof["durable_ledger_after_stop"]=restart_ledger_identity(work)?;
            save_restart(work,&proof)
        })();
        if let Err(id)=result {ledger_preserve_child_failure(work,&mut proof)?;proof["failure_ids"]=json!([id]);let _=save_restart(work,&proof);return Err(id);}
        Ok(())
    }


    use super::*;
    use serde_json::{json, Value};
    use std::{
        collections::{BTreeMap, VecDeque}, fs, io::{Read, Write},
        os::unix::process::ExitStatusExt, path::{Path, PathBuf}, process::{Child, Command, ExitStatus, Stdio},
        sync::Mutex, thread, time::{Duration, Instant},
    };
    const ELF: &str = "473767d296f1cc2f86d0f84312da31f4aa031e195437ef7f5d348ad3f5e0bee6";
    const EXE: &str = "/opt/ferriprint-windows-local-qa/ferriprint";
    const URI: &str = "ipp://127.0.0.1:18634/ipp/print/windows-local-qa";
    const CANARY: i32 = 0x46504f31;
    const LIMIT: usize = 256 * 1024;
    const LINE_LIMIT: usize = 1024 * 1024;
    type Result<T, E = &'static str> = std::result::Result<T, E>;

    struct LimitedWriter<W> { inner:W, used:usize }
    impl<W:Write> Write for LimitedWriter<W> {
        fn write(&mut self,bytes:&[u8])->std::io::Result<usize> {
            if self.used+bytes.len()>LIMIT*8 {return Err(std::io::ErrorKind::FileTooLarge.into());}
            let n=self.inner.write(bytes)?;self.used+=n;Ok(n)
        }
        fn flush(&mut self)->std::io::Result<()> {self.inner.flush()}
    }
    fn status_path(work: &Path) -> PathBuf { work.join("proof/native-observer.json") }
    fn bounded_file(path:&Path,limit:usize)->Result<Vec<u8>> {
        let file=fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path).map_err(|_|"CONTROL_READ")?;
        let mut bytes=Vec::new();
        file.take(limit as u64+1).read_to_end(&mut bytes).map_err(|_|"CONTROL_READ")?;
        if bytes.len()>limit {return Err("CONTROL_LIMIT");} Ok(bytes)
    }
    fn save(work: &Path, value: &Value) -> Result<()> {
        let tmp = work.join("native-observer.pending");
        let file = fs::OpenOptions::new().write(true).create(true).truncate(true)
            .mode(0o600).open(&tmp).map_err(|_| "STATUS_WRITE")?;
        let mut writer=LimitedWriter{inner:std::io::BufWriter::new(file),used:0};
        serde_json::to_writer_pretty(&mut writer,value).map_err(|_|"STATUS_ENCODING_OR_LIMIT")?;
        writer.flush().map_err(|_|"STATUS_WRITE")?;
        fs::rename(tmp, status_path(work)).map_err(|_| "STATUS_WRITE")
    }
    fn number(status: &str, key: &str) -> Result<u32> {
        status.lines().find_map(|line| line.strip_prefix(key))
            .and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok())
            .ok_or("PROC_STATUS")
    }
    fn tasks(pid: u32) -> Result<BTreeMap<u32, u32>> {
        let mut result = BTreeMap::new();
        for entry in fs::read_dir(format!("/proc/{pid}/task")).map_err(|_| "PROC_TASKS")? {
            let entry = entry.map_err(|_| "PROC_TASKS")?;
            let tid: u32 = entry.file_name().to_str().and_then(|v| v.parse().ok()).ok_or("PROC_TASKS")?;
            let text = fs::read_to_string(entry.path().join("status")).map_err(|_| "PROC_TASKS")?;
            if number(&text, "Tgid:")? != pid || number(&text, "Uid:")? != 994 { return Err("THREAD_IDENTITY"); }
            result.insert(tid, number(&text, "TracerPid:")?);
        }
        if result.is_empty() { return Err("NO_THREADS"); }
        Ok(result)
    }
    fn observe_strace_diagnostic(counts:&mut BTreeMap<&'static str,u64>,text:&str)->bool {
        if !text.starts_with("strace:"){return false;}
        // Literal-pattern observations only: never infer or extract an errno,
        // PID, path, filename, error text or buffer from the diagnostic.
        let id=if text.contains("Input/output error") {"STRACE_LITERAL_EIO_TEXT"}
            else if text.contains("Operation not permitted") {"STRACE_LITERAL_EPERM_TEXT"}
            else if text.contains("No such process") {"STRACE_LITERAL_ESRCH_TEXT"}
            else if text.contains("ptrace")||text.contains("PTRACE_") {"STRACE_LITERAL_PTRACE_OTHER"}
            else {"STRACE_LITERAL_DIAGNOSTIC_OTHER"};
        let count=counts.entry(id).or_default();*count=count.saturating_add(1);true
    }
    const LOSS_TID_LIMIT: usize = 128;
    const LOSS_STATUS_LIMIT: usize = 16 * 1024;
    fn loss_u32(value:&[u8])->Option<u32> {
        let token=value.split(|v|v.is_ascii_whitespace()).find(|v|!v.is_empty())?;
        token.iter().try_fold(0u32,|number,byte|{
            if !byte.is_ascii_digit(){return None;}
            number.checked_mul(10)?.checked_add(u32::from(*byte-b'0'))
        })
    }
    pub(super) fn loss_status_projection(bytes:&[u8])->Value {
        let mut tid=None;let mut tracer_pid=None;let mut tgid=None;let mut uid=None;let mut state_code=None;
        // Only complete, bounded lines and explicitly selected numeric fields survive.
        for line in bytes[..bytes.len().min(LOSS_STATUS_LIMIT)].split_inclusive(|v|*v==b'\n') {
            if !line.ends_with(b"\n"){continue;}
            if let Some(value)=line.strip_prefix(b"Pid:"){tid=loss_u32(value);}
            else if let Some(value)=line.strip_prefix(b"TracerPid:"){tracer_pid=loss_u32(value);}
            else if let Some(value)=line.strip_prefix(b"Tgid:"){tgid=loss_u32(value);}
            else if let Some(value)=line.strip_prefix(b"Uid:"){uid=loss_u32(value);}
            else if let Some(value)=line.strip_prefix(b"State:") {
                state_code=value.split(|v|v.is_ascii_whitespace()).find(|v|!v.is_empty())
                    .filter(|v|v.len()==1 && matches!(v[0],
                        b'R'|b'S'|b'D'|b'Z'|b'T'|b't'|b'X'|b'x'|b'K'|b'W'|b'P'|b'I'))
                    .map(|v|v[0] as char);
            }
        }
        json!({"tid":tid,"tracer_pid":tracer_pid,"tgid":tgid,"uid":uid,"state_code":state_code})
    }
    // Read-stage IDs: 0 = no I/O failure, 1 = open, 2 = read. A missing OS errno stays null.
    pub(super) fn loss_status_read_projection(bytes:&[u8],eof:bool,stage:u8,error:Option<&std::io::Error>)->Value {
        json!({"fields":loss_status_projection(bytes),
            "read_failed":error.is_some(),"read_stage_id":stage,
            "read_errno":error.and_then(|v|v.raw_os_error()),
            "status_bytes_read":bytes.len().min(LOSS_STATUS_LIMIT),
            "status_complete":eof && bytes.len()<=LOSS_STATUS_LIMIT && error.is_none(),
            "status_limit_reached":bytes.len()>=LOSS_STATUS_LIMIT,
            "status_may_be_truncated":bytes.len()>LOSS_STATUS_LIMIT || (bytes.len()==LOSS_STATUS_LIMIT && !eof)})
    }
    fn loss_id_projection(ids:impl Iterator<Item=u32>)->Value {
        let mut retained=Vec::with_capacity(ids.size_hint().1.unwrap_or(LOSS_TID_LIMIT).min(LOSS_TID_LIMIT));
        let mut count=0u64;
        for tid in ids {
            if retained.len()<LOSS_TID_LIMIT{retained.push(tid);}
            count=count.saturating_add(1);
        }
        let truncated=count>retained.len() as u64;
        json!({"ids":retained,"count":count,"truncated":truncated})
    }
    // Retain trigger mismatches first so a high-numbered loss cannot hide behind the cap.
    fn loss_current_threads<'a>(current:&'a BTreeMap<u32,u32>,tracer:u32)->impl Iterator<Item=(&'a u32,&'a u32)> {
        current.iter().filter(move|(_,value)|**value!=tracer)
            .chain(current.iter().filter(move|(_,value)|**value==tracer))
    }
    pub(super) fn loss_thread_set_projection(baseline:&BTreeMap<u32,u32>,current:&BTreeMap<u32,u32>,tracer:u32)->Value {
        json!({"phase_id":"AT_COVERAGE_LOSS_GUARD","tid_limit":LOSS_TID_LIMIT,"status_byte_limit":LOSS_STATUS_LIMIT,
            "expected_tracer_pid":tracer,"current_mismatches_first":true,
            "baseline":loss_id_projection(baseline.keys().copied()),
            "current":loss_id_projection(loss_current_threads(current,tracer).map(|(tid,_)|*tid)),
            "added":loss_id_projection(current.keys().copied().filter(|tid|!baseline.contains_key(tid))),
            "missing":loss_id_projection(baseline.keys().copied().filter(|tid|!current.contains_key(tid))),
            "trigger_mismatch_count":current.values().filter(|value|**value!=tracer).count()})
    }
    fn loss_thread_resample(pid:u32,tid:u32,trigger_tracer_pid:u32)->Value {
        let mut bytes=[0u8;LOSS_STATUS_LIMIT];let mut used=0;let mut eof=false;let mut stage=0;let mut error=None;
        match fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW)
            .open(format!("/proc/{pid}/task/{tid}/status")) {
            Ok(mut file)=>{
                while used<LOSS_STATUS_LIMIT {
                    match file.read(&mut bytes[used..]) {
                        Ok(0)=>{eof=true;break;},
                        Ok(n)=>used+=n,
                        Err(value)=>{stage=2;error=Some(value);break;},
                    }
                }
            },
            Err(value)=>{stage=1;error=Some(value);},
        }
        json!({"trigger_tid":tid,"trigger_tracer_pid":trigger_tracer_pid,
            "resampled_numeric_status":loss_status_read_projection(&bytes[..used],eof,stage,error.as_ref())})
    }
    fn thread_coverage_loss_snapshot(pid:u32,baseline:&BTreeMap<u32,u32>,current:&BTreeMap<u32,u32>,tracer:u32)->Value {
        let mut snapshot=loss_thread_set_projection(baseline,current,tracer);
        snapshot["resampled_current_threads"]=Value::Array(loss_current_threads(current,tracer).take(LOSS_TID_LIMIT)
            .map(|(tid,observed)|loss_thread_resample(pid,*tid,*observed)).collect());
        snapshot
    }
    #[derive(Clone,Copy,PartialEq,Eq)]
    struct ProcessSession {pid:u32,sid:u32,pgid:u32}
    impl ProcessSession {
        fn read(pid:u32)->Result<Self> {
            let target=i32::try_from(pid).map_err(|_|"SESSION_PID_RANGE")?;
            let sid=unsafe {libc::getsid(target)};
            if sid<0{return Err("SESSION_SID_READ");}
            let pgid=unsafe {libc::getpgid(target)};
            if pgid<0{return Err("SESSION_PGID_READ");}
            Ok(Self{pid,sid:sid as u32,pgid:pgid as u32})
        }
        fn projection(self)->Value {json!({"pid":self.pid,"sid":self.sid,"pgid":self.pgid})}
        fn isolated_from(self,frontend:Self)->bool {
            self.pid==self.sid && self.pid==self.pgid &&
                self.sid!=frontend.sid && self.pgid!=frontend.pgid
        }
        fn inherits(self,observer:Self)->bool {self.sid==observer.sid && self.pgid==observer.pgid}
    }
    #[derive(Default)]
    struct Flow {
        request: Vec<u8>, response: Vec<u8>, pending: VecDeque<Value>,
        requests: u64, responses: u64,
    }
    fn io_guard_shape(call:&str,fd:i32,returned:i64,first:&str)->Value {
        let call=match call {"read"=>"READ","readv"=>"READV","recvfrom"=>"RECVFROM","recvmsg"=>"RECVMSG",
            "write"=>"WRITE","writev"=>"WRITEV","sendto"=>"SENDTO","sendmsg"=>"SENDMSG",_=>"OTHER"};
        let annotation=if first.contains("<TCP:[") || first.contains("<TCP:") {"TCP"}
            else if first.contains("<TCPv6:") {"TCPv6"}else if first.contains("<socket:") {"socket"}
            else if !first.contains('<') {"missing"}else {"other"};
        let rest=first.split_once("<TCP:[").or_else(||first.split_once("<TCPv6:[")).map(|v|v.1);
        let body=rest.and_then(|v|v.split_once("]>")).map(|v|v.0);
        let pair=body.and_then(|v|v.split_once("->"));
        let local=pair.and_then(|v|v.0.parse::<std::net::SocketAddr>().ok());
        let peer=pair.and_then(|v|v.1.parse::<std::net::SocketAddr>().ok());
        json!({"syscall_id":call,"fd":fd,"returned_bytes":returned,"annotation_class_id":annotation,
            "TCP_marker_present":rest.is_some(),"endpoint_bracket_terminator_present":body.is_some(),
            "endpoint_separator_present":pair.is_some(),"local_endpoint_parse_ok":local.is_some(),
            "peer_endpoint_parse_ok":peer.is_some(),"peer_port_positive":peer.is_some_and(|v|v.port()>0)})
    }
    struct Recorder {
        root:u32,phase:&'static str,flows:BTreeMap<u64,Flow>,pending:BTreeMap<u32,String>,
        origins:KernelFdOrigins,allowed_threads:std::collections::BTreeSet<u32>,baseline_threads:Vec<u32>,
        attachment_proven:bool,thread_coverage_loss:Option<Value>,tracer_lifecycle:Option<Value>,session_ownership:Option<Value>,
        startup_proof:Option<Value>,exec_identity:Option<Value>,startup_identity_and_state_proven:bool,restart_preservation_required:bool,
        io_guard_records:Vec<Value>,io_guard_total:u64,strace_diagnostic_literal_counts:BTreeMap<&'static str,u64>,
        failure_shapes:BTreeMap<String,u64>,
        records:Vec<Value>,canary:Option<Value>,failures:Vec<&'static str>,read_bytes:u64,write_bytes:u64,dropped:u64,unparsed:u64,
        interim:u64,unexpected:bool,runtime:tokio::runtime::Runtime,
    }
    fn attr<'a>(m: &'a IppRequestResponse, key: &str) -> Option<&'a IppValue> {
        m.attributes().groups().iter().flat_map(|g| g.attributes())
            .find(|v| v.name().as_str() == key).map(|v| v.value())
    }
    fn selected(v: &IppValue, depth: usize) -> Result<Value> {
        if depth > 4 { return Err("PROJECTION_DEPTH"); }
        Ok(match v {
            IppValue::Integer(n) | IppValue::Enum(n) => json!(n),
            IppValue::Boolean(v) => json!(v),
            IppValue::Keyword(v) | IppValue::MimeMediaType(v) => json!(v.as_str()),
            IppValue::Resolution { cross_feed, feed, units } =>
                json!({"x":cross_feed,"y":feed,"units":units}),
            IppValue::Array(v) => {
                if v.len()>128 {return Err("PROJECTION_ARRAY_LIMIT");}
                Value::Array(v.iter().map(|v|selected(v,depth+1)).collect::<Result<_>>()?)
            },
            IppValue::Collection(v) => {
                let mut result=serde_json::Map::new();
                for (key,value) in v.iter().filter(|(k,_)|matches!(k.as_str(),
                    "media-size"|"x-dimension"|"y-dimension"|"media-top-margin"|"media-bottom-margin"|
                    "media-left-margin"|"media-right-margin"|"media-source"|"media-type")) {
                    result.insert(key.to_string(),selected(value,depth+1)?);
                }
                Value::Object(result)
            },
            _ => json!({"value_type_id":"OTHER"}),
        })
    }
    fn keywords(v: &IppValue) -> Result<Vec<String>> {
        match v {
            IppValue::Keyword(v) if v.len()<=128 &&
                v.as_str().bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-') => Ok(vec![v.to_string()]),
            IppValue::Array(v) if v.len()<=128 => {
                let mut result=Vec::new();
                for value in v {result.extend(keywords(value)?);}
                Ok(result)
            },
            _ => Err("REQUESTED_ATTRIBUTE_NAME"),
        }
    }
    impl Recorder {
        fn new(root:u32)->Result<Self> {
            Ok(Self{root,phase:"canary",flows:BTreeMap::new(),pending:BTreeMap::new(),origins:KernelFdOrigins::new(&[])?,
                allowed_threads:std::collections::BTreeSet::from([root]),baseline_threads:Vec::new(),attachment_proven:false,
                thread_coverage_loss:None,tracer_lifecycle:None,session_ownership:None,startup_proof:None,exec_identity:None,
                startup_identity_and_state_proven:false,restart_preservation_required:true,io_guard_records:Vec::new(),io_guard_total:0,
                strace_diagnostic_literal_counts:BTreeMap::new(),failure_shapes:BTreeMap::new(),records:Vec::new(),canary:None,failures:Vec::new(),
                read_bytes:0,write_bytes:0,dropped:0,unparsed:0,interim:0,unexpected:false,
                runtime:tokio::runtime::Builder::new_current_thread().build().map_err(|_|"PARSER_RUNTIME")?})
        }
        fn fail(&mut self, id: &'static str) { self.restart_preservation_required=true; if !self.failures.contains(&id) { self.failures.push(id); } }
        // Fixed-shape classification of a rejected trace line. Only enumerations and
        // counts leave the process; no syscall text, path, address or payload is kept.
        fn note_shape(&mut self, id:&'static str, text:&str) {
            let shape = if text.starts_with("[pid ") { "PID_BRACKET" }
                else if text.starts_with("<... ") { "RESUMED" }
                else if text.contains("<unfinished ...>") { "UNFINISHED" }
                else { "OTHER" };
            let grammar = if !text.contains('(') { "NO_LPAREN" }
                else if !text.contains(')') { "NO_RPAREN" }
                else if text.contains(") = ") { "TIGHT_EQUALS" }
                else if text.contains(") =") { "PADDED_EQUALS" }
                else { "NO_EQUALS" };
            let call:&str = text.split_once('(').map(|v|v.0.rsplit_once(']').map(|x|x.1.trim()).unwrap_or(v.0.trim()))
                .map(|name| match name {
                    "read"|"readv"|"recvfrom"|"recvmsg"|"write"|"writev"|"sendto"|"sendmsg" => "IO",
                    "socket"|"socketpair"|"bind"|"listen"|"connect"|"accept"|"accept4" => "SOCKET",
                    "open"|"openat"|"openat2"|"creat"|"close"|"close_range"|"dup"|"dup2"|"dup3"|"fcntl" => "FD",
                    "clone"|"clone3"|"fork"|"vfork"|"unshare"|"execve"|"execveat" => "PROC",
                    "pipe"|"pipe2"|"eventfd"|"eventfd2"|"signalfd"|"signalfd4"|"timerfd_create"|"epoll_create"|"epoll_create1" => "HANDLE",
                    _ => "UNKNOWN_SYSCALL" }).unwrap_or("NO_CALL");
            // Fixed-shape facts about the first angle annotation on the line. Only
            // enumerations leave the process: no path, inode, address or payload.
            let ann = text.find('<').map(|at| {
                let rest=&text[at..];
                let closed=rest.find('>').is_some();
                let inner=rest.strip_prefix('<').and_then(|v|v.split_once('>').map(|x|x.0)).unwrap_or("");
                let nested=inner.contains('<');
                let prefix=inner.split_once(':').map(|v|v.0).unwrap_or("");
                let kind = if inner.is_empty() { "EMPTY" }
                    else if nested { "NESTED" }
                    else if !closed { "UNCLOSED" }
                    else { match prefix {
                        "TCP"|"UDP"|"TCPv6"|"UDPv6"|"socket"|"UNIX"|"UNIX-STREAM"|"UNIX-DGRAM"|"NETLINK"|"PACKET" => "SOCKET_PREFIX",
                        _ if prefix.is_empty() => "PLAIN",
                        _ => "NAMED_PREFIX" } };
                let tag = if inner.contains("<char ") { "CHAR_TAG" }
                    else if inner.contains("<block ") { "BLOCK_TAG" }
                    else if nested { "OTHER_NEST" } else { "NO_TAG" };
                format!("{kind}/{tag}")
            }).unwrap_or_else(|| "NO_ANGLE".to_string());
            let bucket=format!("{id}|{shape}|{grammar}|{call}|{ann}");
            let entry=self.failure_shapes.entry(bucket).or_insert(0u64);
            if *entry!=u64::MAX { *entry+=1; }
        }
        fn ipp(&self, body: &[u8]) -> Result<IppRequestResponse> {
            let mut input = body;
            let parsed = self.runtime.block_on(read_metadata(&mut input, LIMIT)).map_err(|_| "IPP_METADATA")?;
            if !input.is_empty() { return Err("IPP_EXTRA_BYTES"); }
            Ok(parsed)
        }
        fn request(&mut self, start: &str, headers: &BTreeMap<String,String>, body: &[u8]) -> Result<Value> {
            let mut parts = start.split_whitespace();
            let method = parts.next().ok_or("HTTP_START")?;
            let target = parts.next().ok_or("HTTP_START")?;
            let version = parts.next().ok_or("HTTP_START")?;
            if !matches!(version,"HTTP/1.1"|"HTTP/1.0") || parts.next().is_some() { return Err("HTTP_VERSION"); }
            let mut result = json!({
                "phase":self.phase,
                "method_id":match method { "POST"=>"POST","GET"=>"GET","HEAD"=>"HEAD",_=>"OTHER" },
                "path_match_id":if target == "/ipp/print/windows-local-qa" {"OWNED"} else {"OTHER"},
                "query_present":target.contains('?'),
                "authority_match_id":match headers.get("host") { Some(v) if v=="127.0.0.1:18634"=>"OWNED",None=>"MISSING",_=>"OTHER" },
                "authorization_header_present":headers.contains_key("authorization") || headers.contains_key("proxy-authorization"),
                "operation":null,"request_id":null,"ipp_version":null,
                "request_printer_uri_match_id":"UNAVAILABLE","requested_attributes":[],
                "document_format_id":null,"request_complete":true
            });
            if headers.get("content-type").is_some_and(|v| v.split(';').next().is_some_and(|v| v.trim().eq_ignore_ascii_case("application/ipp"))) {
                if body.len() < 8 { return Err("IPP_HEADER"); }
                let op = i16::from_be_bytes([body[2],body[3]]);
                result["operation"] = json!(op);
                result["request_id"] = json!(i32::from_be_bytes([body[4],body[5],body[6],body[7]]));
                result["ipp_version"] = json!([body[0],body[1]]);
                if !matches!(op,4|9|10|11|0x15) { self.unexpected = true; }
                if op == 11 {
                    let m = self.ipp(body)?;
                    result["request_printer_uri_match_id"] = json!(match attr(&m,"printer-uri") {
                        Some(IppValue::Uri(v)) if v.as_str()==URI => "OWNED", None=>"MISSING",_=>"OTHER"
                    });
                    if let Some(v) = attr(&m,"requested-attributes") { result["requested_attributes"]=json!(keywords(v)?); }
                    result["document_format_id"] = json!(match attr(&m,"document-format") {
                        None=>"MISSING", Some(IppValue::MimeMediaType(v)) => match v.as_str() {
                            "application/pdf"=>"PDF","image/pwg-raster"=>"PWG","image/urf"=>"URF",
                            "image/jpeg"=>"JPEG","image/png"=>"PNG","application/octet-stream"=>"OCTET_STREAM",_=>"OTHER"
                        }, _=>"OTHER"
                    });
                }
            }
            Ok(result)
        }
        fn bytes(&mut self, flow_key: u64, read: bool, bytes: &[u8]) -> Result<()> {
            if self.flows.len() >= 64 && !self.flows.contains_key(&flow_key) { return Err("FLOW_LIMIT"); }
            let mut flow = self.flows.remove(&flow_key).unwrap_or_default();
            let buffer = if read { &mut flow.request } else { &mut flow.response };
            if buffer.len()+bytes.len() > LIMIT*2 { return Err("HTTP_BUFFER_LIMIT"); }
            buffer.extend_from_slice(bytes);
            loop {
                let buffer = if read { &flow.request } else { &flow.response };
                let head = !read && flow.pending.front().is_some_and(|v| v["method_id"]=="HEAD");
                let Some((used,start,headers,body,interim)) = frame(buffer,!read,head)? else { break };
                if interim { self.interim+=1; }
                else if read {
                    let mut req = self.request(&start,&headers,&body)?;
                    req["peer_match_id"]=json!("NOT_RETAINED_KERNEL_ORIGIN_ONLY");
                    flow.requests+=1;
                    if flow.pending.len() >= 32 { return Err("PIPELINE_LIMIT"); }
                    flow.pending.push_back(req);
                } else {
                    let status: u16 = start.split_whitespace().nth(1).and_then(|v|v.parse().ok()).ok_or("HTTP_STATUS")?;
                    let mut req = flow.pending.pop_front().ok_or("RESPONSE_WITHOUT_REQUEST")?;
                    req["http_status"]=json!(status);
                    req["response_complete"]=json!(true);
                    req["response_ipp_status"]=Value::Null;
                    if req["operation"]==11 && headers.get("content-type").is_some_and(|v|v.split(';').next().is_some_and(|v|v.trim().eq_ignore_ascii_case("application/ipp"))) {
                        let m = self.ipp(&body)?;
                        req["response_ipp_status"]=json!(m.header().operation_or_status);
                        req["response_request_id_matches"]=json!(Some(m.header().request_id as i64)==req["request_id"].as_i64());
                        let keys = ["media-supported","media-col-database","media-size-supported",
                            "print-quality-supported","sides-supported","printer-resolution-supported",
                            "print-color-mode-supported","color-supported","document-format-supported",
                            "pwg-raster-document-resolution-supported","pwg-raster-document-type-supported","urf-supported"];
                        let mut projection=serde_json::Map::new();
                        for key in keys { if let Some(v)=attr(&m,key) { projection.insert(key.to_string(),selected(v,0)?); } }
                        req["selected_response_attributes"]=Value::Object(projection);
                    }
                    flow.responses+=1;
                    if self.records.len() >= 128 { return Err("TRANSACTION_LIMIT"); }
                    self.records.push(req);
                }
                let buffer=if read {&mut flow.request} else {&mut flow.response};
                buffer.drain(..used);
            }
            if read { self.read_bytes+=bytes.len() as u64; } else { self.write_bytes+=bytes.len() as u64; }
            self.flows.insert(flow_key,flow);
            Ok(())
        }
                fn line(&mut self,line:&str)->Result<()> {
                    let (tid,text)=prefix(line,self.root)?;
                    if observe_strace_diagnostic(&mut self.strace_diagnostic_literal_counts,text){return Ok(());}
                    if !self.allowed_threads.contains(&tid) && !text.starts_with("strace:") {
                        let status=fs::read_to_string(format!("/proc/{}/task/{tid}/status",self.root)).map_err(|_|"THREAD_IDENTITY_UNAVAILABLE")?;
                        if number(&status,"Tgid:")?!=self.root || number(&status,"Uid:")?!=994 {return Err("NON_FRONTEND_TRACE_THREAD");}
                        self.allowed_threads.insert(tid);
                    }
                    if text.starts_with("strace:") || text.starts_with("---") || text.starts_with("+++"){return Ok(());}
                    if let Some(at)=text.find("<unfinished ...>") {
                        if self.pending.len()>=128 || text.len()>LINE_LIMIT {return Err("PENDING_LIMIT");}
                        self.origins.note_entry(tid,&text[..at])?;
                        if self.pending.insert(tid,text[..at].to_string()).is_some(){return Err("DOUBLE_PENDING");}
                        return Ok(());
                    }
                    let complete;
                    let text=if text.starts_with("<... ") {
                        let Some(start)=self.pending.remove(&tid) else {self.unparsed+=1;return Err("UNMATCHED_RESUMED");};
                        let (marker,tail)=text.split_once("resumed>").ok_or("RESUMED_SYNTAX")?;
                        if marker.strip_prefix("<... ").map(str::trim_end)!=start.split_once('(').map(|v|v.0){return Err("RESUMED_CALL_CONFLICT");}
                        self.origins.note_resume(tid)?;
                        complete=start+tail;complete.as_str()
                    }else{text};
                    let Some((call,args))=text.split_once('(')else{return Ok(());};
                    let (args,result_text)=split_result(args).ok_or("SYSCALL_SYNTAX")?;
                    let result_text=result_text.trim();
                    let returned=if result_text.starts_with("? ") {-1}else{
                        let token=result_text.split(|v:char|v=='<' || v.is_ascii_whitespace()).next().ok_or("SYSCALL_RESULT")?;
                        if token=="-1" {-1}else{i64::try_from(kernel_u64(token)?).map_err(|_|"SYSCALL_RESULT")?}
                    };
                    let event=match self.origins.event(call,args,returned,result_text) {
                        Ok(event)=>event,
                        Err(id)=>{
                            if id=="IO_SOCKET_UNCLASSIFIED" {
                                self.io_guard_total+=1;
                                if self.io_guard_records.len()<8 {
                                    let first=kernel_fields(args,false)?.get(0)?;
                                    self.io_guard_records.push(io_guard_shape(call,kernel_fd(first)?.fd,returned,first));
                                }
                            }
                            return Err(id);
                        }
                    };
                    if event.verify_exec {
                        let identity=verified_frontend_identity(self.root)?;
                        if identity["supplementary_groups"].as_array().is_none_or(|v|!v.is_empty()){return Err("FRONTEND_SUPPLEMENTARY_GROUPS");}
                        self.origins.confirm_published_exec()?;
                        self.exec_identity=Some(identity);
                    }
                    for generation in event.retired_targets {
                        if self.flows.get(&generation).is_some_and(|v|!v.request.is_empty() || !v.response.is_empty() || !v.pending.is_empty()) {
                            return Err("CLOSED_INCOMPLETE_HTTP");
                        }
                        self.flows.remove(&generation);
                    }
                    let Some(generation)=event.io_target else{return Ok(());};
                    let read=matches!(call,"read"|"readv"|"recvfrom"|"recvmsg");
                    if !read && !matches!(call,"write"|"writev"|"sendto"|"sendmsg"){return Err("KERNEL_IO_ROLE_CONFLICT");}
                    if args.contains("MSG_PEEK"){return Ok(());}
                    let n=usize::try_from(returned).map_err(|_|"BYTE_COUNT")?;
                    let data=io_bytes(args,matches!(call,"readv"|"writev"|"recvmsg"|"sendmsg"),n)?;
                    if n>data.len(){return Err("BYTE_COUNT");}
                    self.bytes(generation,read,&data[..n])
                }
        fn canary_ok(&self) -> bool {
            self.failures.is_empty() && self.records.len()==1 && self.read_bytes>0 && self.write_bytes>0 &&
                self.records.first().is_some_and(|v| v["request_id"]==CANARY && v["operation"]==11 &&
                    v["http_status"]==200 && v["response_ipp_status"]==0 &&
                    v["response_request_id_matches"]==true && v["method_id"]=="POST" &&
                    v["path_match_id"]=="OWNED" && v["authority_match_id"]=="OWNED" &&
                    v["request_printer_uri_match_id"]=="OWNED" && v["authorization_header_present"]==false) &&
                self.flows.values().all(|f|f.request.is_empty() && f.response.is_empty() && f.pending.is_empty())
        }
        fn native(&mut self) -> Result<()> {
            if !self.canary_ok() { return Err("CANARY_COVERAGE"); }
            self.canary=self.records.pop(); self.phase="native";
            if let Some(canary)=self.canary.as_mut() {
                canary["observed_socket_read_bytes"]=json!(self.read_bytes);
                canary["observed_socket_write_bytes"]=json!(self.write_bytes);
                canary["interim_100_count"]=json!(self.interim);
            }
            self.read_bytes=0;self.write_bytes=0;self.interim=0;
            Ok(())
        }
        fn projection(&self,armed:bool,disarmed:bool,threads:&BTreeMap<u32,u32>,tracer:u32) -> Value {
            let buffered=self.flows.values().map(|v|v.request.len()+v.response.len()).sum::<usize>();
            let pending_http=self.flows.values().map(|v|v.pending.len()).sum::<usize>();
            json!({"phase":self.phase,"armed":armed,"disarmed":disarmed,
                "frontend_pid":self.root,"frontend_uid":994,"frontend_elf_sha256":ELF,
                "tracer_pid":tracer,"thread_ids":threads.keys().collect::<Vec<_>>(),
                "all_threads_traced":armed && threads.values().all(|v|*v==tracer),
                "baseline_untraced_thread_ids":self.baseline_threads,
                "full_thread_attachment_proven_before_canary_and_native":self.attachment_proven,
                "thread_coverage_loss":self.thread_coverage_loss,"tracer_lifecycle":self.tracer_lifecycle,
                "session_ownership":self.session_ownership,
                "io_socket_unclassified":{"record_limit":8,"total_guard_calls":self.io_guard_total,"records":self.io_guard_records,"records_truncated":self.io_guard_total>self.io_guard_records.len() as u64},
                "kernel_fd_origins":self.origins.projection(),"startup_proof":self.startup_proof,
                "startup_identity_and_state_proven":self.startup_identity_and_state_proven,"restart_preservation_required":self.restart_preservation_required,
                "frontend_exec_identity":self.exec_identity,"baseline_scope_id":"GATED_PREEXEC_SHIM_UID994_NO_SOCKETS",
                "strace_diagnostic_semantics_id":"LITERAL_PATTERN_OBSERVATIONS_NOT_OS_ERRNO_FACTS",
                "strace_diagnostic_literal_counts":{
                    "STRACE_LITERAL_EIO_TEXT":self.strace_diagnostic_literal_counts.get("STRACE_LITERAL_EIO_TEXT").copied().unwrap_or(0),
                    "STRACE_LITERAL_EPERM_TEXT":self.strace_diagnostic_literal_counts.get("STRACE_LITERAL_EPERM_TEXT").copied().unwrap_or(0),
                    "STRACE_LITERAL_ESRCH_TEXT":self.strace_diagnostic_literal_counts.get("STRACE_LITERAL_ESRCH_TEXT").copied().unwrap_or(0),
                    "STRACE_LITERAL_PTRACE_OTHER":self.strace_diagnostic_literal_counts.get("STRACE_LITERAL_PTRACE_OTHER").copied().unwrap_or(0),
                    "STRACE_LITERAL_DIAGNOSTIC_OTHER":self.strace_diagnostic_literal_counts.get("STRACE_LITERAL_DIAGNOSTIC_OTHER").copied().unwrap_or(0)},
                "observed_frontend_thread_ids":self.allowed_threads,
                "canary_request_id":CANARY,"canary":self.canary,
                "native_transactions":self.records,"native_socket_read_bytes":self.read_bytes,
                "native_socket_write_bytes":self.write_bytes,"interim_100_count":self.interim,
                "dropped_trace_bytes":self.dropped,"unparsed_calls":self.unparsed,
                "unparsed_shape_counts":self.failure_shapes,
                "buffered_http_bytes":buffered,"pending_http_requests":pending_http,
                "failure_ids":self.failures,"unexpected_mutating_or_unknown_operation":self.unexpected,
                "capture_complete":self.startup_identity_and_state_proven && !self.restart_preservation_required && self.origins.listener_ready() && disarmed && self.canary.is_some() && self.failures.is_empty() &&
                    self.dropped==0 && self.unparsed==0 && buffered==0 && pending_http==0,
                "raw_trace_headers_files_buffers_exported":false,
                "response_scope_id":"FRONTEND_TCP_WRITE_COMPLETION_NOT_WINDOWS_CONSUMPTION_OR_ACK",
                "trace_changes_scheduling_not_a_fix_or_original_cause_proof":true})
        }
    }
    // strace right-aligns the result column, so the closing parenthesis of the
    // argument list can be followed by one or more spaces before "= ". Splitting
    // on the fixed string ") = " therefore rejects every padded line. Accept any
    // amount of padding while still requiring the argument list to end in ")" and
    // the result to start after "= ".
    fn split_result(args:&str)->Option<(&str,&str)> {
        let mut from=args.len();
        while let Some(at)=args[..from].rfind(") ") {
            if let Some(rest)=args[at+2..].trim_start_matches(' ').strip_prefix("= ") {return Some((&args[..at],rest));}
            from=at;
        }
        None
    }
    fn prefix(line:&str,root:u32)->Result<(u32,&str)> {
        let line=line.trim_end_matches(['\r','\n']);
        if let Some(rest)=line.strip_prefix("[pid ") {
            let (tid,text)=rest.split_once(']').ok_or("TID_PREFIX")?;
            return Ok((tid.trim().parse().map_err(|_|"TID_PREFIX")?,text.trim_start()));
        }
        if let Some((tid,text))=line.split_once(' ') {
            if let Ok(tid)=tid.parse() { return Ok((tid,text.trim_start())); }
        }
        Ok((root,line))
    }
    fn quoted(text:&str)->Result<(Vec<u8>,usize)> {
        let start=text.find('"').ok_or("IO_STRING")?;
        let mut at=start+1;let b=text.as_bytes();let mut out=Vec::new();
        while at<b.len() {
            let c=b[at];at+=1;
            if c==b'"' {
                if text[at..].starts_with("...") { return Err("IO_TRUNCATED"); }
                return Ok((out,at));
            }
            let v=if c!=b'\\' {c} else {
                let c=*b.get(at).ok_or("IO_ESCAPE")?;at+=1;
                match c {
                    b'x'=>{let h=text.get(at..at+2).ok_or("IO_ESCAPE")?;
                        let v=u8::from_str_radix(h,16).map_err(|_|"IO_ESCAPE")?;at+=2;v},
                    b'0'..=b'7'=>{let mut n=(c-b'0') as u16;
                        for _ in 0..2 {if let Some(c @ b'0'..=b'7')=b.get(at) {n=n*8+(c-b'0') as u16;at+=1;} else {break}}
                        u8::try_from(n).map_err(|_|"IO_ESCAPE")?},
                    b'n'=>b'\n',b'r'=>b'\r',b't'=>b'\t',b'\\'=>b'\\',b'"'=>b'"',
                    _=>return Err("IO_ESCAPE"),
                }
            };
            if out.len()>=LIMIT {return Err("IO_LIMIT");} out.push(v);
        }
        Err("IO_STRING")
    }
    fn io_bytes(args:&str,vectored:bool,needed:usize)->Result<Vec<u8>> {
        if !vectored {
            let text=args.split_once(',').ok_or("IO_ARGUMENT")?.1.trim_start();
            if !text.starts_with('"') {return Err("IO_POINTER");}
            return quoted(text).map(|v|v.0);
        }
        let mut out=Vec::new();let mut rest=args;
        while out.len()<needed {
            let (_,tail)=rest.split_once("iov_base=").ok_or("IO_VECTOR")?;
            if !tail.starts_with('"') {return Err("IO_VECTOR_POINTER");}
            let (data,used)=quoted(tail)?;
            if out.len()+data.len()>LIMIT {return Err("IO_LIMIT");}
            out.extend_from_slice(&data);rest=&tail[used..];
        }
        Ok(out)
    }
    type Frame=(usize,String,BTreeMap<String,String>,Vec<u8>,bool);
    fn frame(raw:&[u8],response:bool,head:bool)->Result<Option<Frame>> {
        let Some(end)=raw.windows(4).position(|v|v==b"\r\n\r\n") else {
            if raw.len()>16384 {return Err("HTTP_HEADER_LIMIT");} return Ok(None);
        };
        if end>16384 {return Err("HTTP_HEADER_LIMIT");}
        let header=std::str::from_utf8(&raw[..end]).map_err(|_|"HTTP_HEADER_ENCODING")?;
        let mut lines=header.split("\r\n");let start=lines.next().ok_or("HTTP_START")?.to_string();
        let mut fields=BTreeMap::new();
        for line in lines {
            let (key,value)=line.split_once(':').ok_or("HTTP_HEADER")?;
            let key=key.to_ascii_lowercase();
            if fields.insert(key.clone(),value.trim().to_string()).is_some() &&
                matches!(key.as_str(),"host"|"content-length"|"transfer-encoding"|"content-type") {return Err("HTTP_DUPLICATE_HEADER");}
        }
        let status=if response {Some(start.split_whitespace().nth(1).and_then(|v|v.parse::<u16>().ok()).ok_or("HTTP_STATUS")?)} else {None};
        if response && !matches!(start.split_whitespace().next(),Some("HTTP/1.1"|"HTTP/1.0")) {return Err("HTTP_VERSION");}
        if status.is_some_and(|v|(101..200).contains(&v)) {return Err("HTTP_INTERIM_UNSUPPORTED");}
        let no_body=head || status.is_some_and(|v|(100..200).contains(&v)||v==204||v==304);
        let at=end+4;
        if no_body {return Ok(Some((at,start,fields,Vec::new(),status==Some(100))));}
        if fields.contains_key("transfer-encoding") && fields.contains_key("content-length") {return Err("HTTP_AMBIGUOUS_LENGTH");}
        if let Some(te)=fields.get("transfer-encoding") {
            if !te.eq_ignore_ascii_case("chunked") {return Err("HTTP_TRANSFER_ENCODING");}
            let mut pos=at;let mut body=Vec::new();
            loop {
                let Some(n)=raw[pos..].windows(2).position(|v|v==b"\r\n") else {return Ok(None)};
                if n>128 {return Err("HTTP_CHUNK");}
                let size=std::str::from_utf8(&raw[pos..pos+n]).map_err(|_|"HTTP_CHUNK")?.split(';').next().ok_or("HTTP_CHUNK")?;
                let size=usize::from_str_radix(size,16).map_err(|_|"HTTP_CHUNK")?;pos+=n+2;
                if size==0 {
                    if raw.get(pos..pos+2)==Some(b"\r\n") {pos+=2;}
                    else if let Some(n)=raw[pos..].windows(4).position(|v|v==b"\r\n\r\n") {if n>16384{return Err("HTTP_TRAILER_LIMIT");} pos+=n+4;}
                    else {return Ok(None);}
                    return Ok(Some((pos,start,fields,body,false)));
                }
                if size>LIMIT || body.len()+size>LIMIT {return Err("HTTP_BODY_LIMIT");}
                if raw.len()<pos+size+2 {return Ok(None);}
                if &raw[pos+size..pos+size+2]!=b"\r\n" {return Err("HTTP_CHUNK");}
                body.extend_from_slice(&raw[pos..pos+size]);pos+=size+2;
            }
        }
        let length=match fields.get("content-length") {
            Some(v)=>v.parse::<usize>().map_err(|_|"HTTP_LENGTH")?,
            None if !response=>0, None=>return Err("HTTP_CLOSE_DELIMITED_UNSUPPORTED"),
        };
        if length>LIMIT{return Err("HTTP_BODY_LIMIT");}
        if raw.len()<at+length{return Ok(None);}
        Ok(Some((at+length,start,fields,raw[at..at+length].to_vec(),false)))
    }
    #[derive(Clone,Copy)]
    pub(super) enum ChildWaitSnapshot {
        Running,
        Exited{code:Option<i32>,signal:Option<i32>},
        Failed{errno:Option<i32>},
    }
    impl ChildWaitSnapshot {
        fn exited(status:&ExitStatus)->Self {Self::Exited{code:status.code(),signal:status.signal()}}
        pub(super) fn from_try_wait(result:&std::io::Result<Option<ExitStatus>>)->Self {
            match result {
                Ok(Some(status))=>Self::exited(status),
                Ok(None)=>Self::Running,
                Err(error)=>Self::Failed{errno:error.raw_os_error()},
            }
        }
        pub(super) fn from_wait(result:&std::io::Result<ExitStatus>)->Self {
            match result {Ok(status)=>Self::exited(status),Err(error)=>Self::Failed{errno:error.raw_os_error()}}
        }
        pub(super) fn projection(self)->Value {
            let (running,code,signal,failed,errno)=match self {
                Self::Running=>(Some(true),None,None,false,None),
                Self::Exited{code,signal}=>(Some(false),code,signal,false,None),
                Self::Failed{errno}=>(None,None,None,true,errno),
            };
            json!({"running":running,"code":code,"signal":signal,"wait_failed":failed,"wait_errno":errno})
        }
    }
    #[derive(Default)]
    struct SignalSends {
        attempts:u64,successes:u64,failures:u64,last_succeeded:Option<bool>,last_errno:Option<i32>,
    }
    impl SignalSends {
        fn observe(&mut self,succeeded:bool,errno:Option<i32>) {
            self.attempts=self.attempts.saturating_add(1);
            if succeeded{self.successes=self.successes.saturating_add(1);}
            else{self.failures=self.failures.saturating_add(1);}
            self.last_succeeded=Some(succeeded);self.last_errno=if succeeded{None}else{errno};
        }
        fn projection(&self,signal:i32)->Value {
            json!({"signal":signal,"attempt_count":self.attempts,"successful_call_count":self.successes,
                "failed_call_count":self.failures,"last_call_succeeded":self.last_succeeded,"last_errno":self.last_errno})
        }
    }
    // Existing wait sites: 1 = observation loop, 2 = initial stop try_wait,
    // 3 = stop-loop try_wait, 4 = post-kill wait. No new wait or signal is introduced.
    const WAIT_OBSERVE:u8=1;const WAIT_BEFORE_STOP:u8=2;const WAIT_STOP_LOOP:u8=3;const WAIT_AFTER_KILL:u8=4;
    #[derive(Default)]
    struct TraceLifecycle {
        before_owned_stop:Option<ChildWaitSnapshot>,last_wait:Option<ChildWaitSnapshot>,last_wait_stage:Option<u8>,
        final_exit:Option<(Option<i32>,Option<i32>)>,wait_failures:u64,sigint:SignalSends,sigkill:SignalSends,
    }
    impl TraceLifecycle {
        fn observe_wait(&mut self,snapshot:ChildWaitSnapshot,stage:u8) {
            if stage==WAIT_BEFORE_STOP && self.before_owned_stop.is_none(){self.before_owned_stop=Some(snapshot);}
            self.last_wait=Some(snapshot);self.last_wait_stage=Some(stage);
            match snapshot {
                ChildWaitSnapshot::Exited{code,signal}=>self.final_exit=Some((code,signal)),
                ChildWaitSnapshot::Failed{..}=>self.wait_failures=self.wait_failures.saturating_add(1),
                ChildWaitSnapshot::Running=>{},
            }
        }
        fn projection(&self)->Value {
            let (code,signal)=self.final_exit.unwrap_or((None,None));
            json!({"before_owned_stop_signal":self.before_owned_stop.map(|snapshot|json!({
                    "phase_id":"BEFORE_OWNED_STOP_SIGNAL","try_wait":snapshot.projection()})),
                "after_owned_stop_handling":{"phase_id":"AFTER_OWNED_STOP_HANDLING",
                    "final_exit_observed":self.final_exit.is_some(),"final_code":code,"final_signal":signal,
                    "last_wait":self.last_wait.map(ChildWaitSnapshot::projection),"last_wait_stage_id":self.last_wait_stage,
                    "wait_failure_count":self.wait_failures,
                    "owned_sigint":self.sigint.projection(libc::SIGINT),"owned_sigkill":self.sigkill.projection(libc::SIGKILL)}})
        }
    }
    struct Trace { child:Child,lifecycle:TraceLifecycle }
    impl Trace {
        fn try_wait(&mut self,stage:u8)->std::io::Result<Option<ExitStatus>> {
            let result=self.child.try_wait();
            self.lifecycle.observe_wait(ChildWaitSnapshot::from_try_wait(&result),stage);
            result
        }
        fn stop(&mut self)->Result<()> {
            if self.try_wait(WAIT_BEFORE_STOP).map_err(|_|"TRACE_WAIT")?.is_some(){return Ok(());}
            let sent=unsafe {libc::kill(self.child.id() as i32,libc::SIGINT)};
            let errno=if sent==0{None}else{std::io::Error::last_os_error().raw_os_error()};
            self.lifecycle.sigint.observe(sent==0,errno);
            let deadline=Instant::now()+Duration::from_secs(3);
            loop {
                if self.try_wait(WAIT_STOP_LOOP).map_err(|_|"TRACE_WAIT")?.is_some(){return Ok(());}
                if Instant::now()>=deadline {
                    let killed=self.child.kill();
                    self.lifecycle.sigkill.observe(killed.is_ok(),killed.as_ref().err().and_then(|v|v.raw_os_error()));
                    let waited=self.child.wait();
                    self.lifecycle.observe_wait(ChildWaitSnapshot::from_wait(&waited),WAIT_AFTER_KILL);
                    return Err("TRACE_DETACH_TIMEOUT");
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    impl Drop for Trace {fn drop(&mut self){let _=self.stop();}}
    const KERNEL_TRACE_SELECTORS:&str="socket,socketpair,bind,listen,connect,accept,accept4,dup,dup2,dup3,fcntl,close,close_range,open,openat,openat2,creat,pipe,pipe2,eventfd,eventfd2,epoll_create,epoll_create1,signalfd,signalfd4,timerfd_create,clone,clone3,fork,vfork,unshare,execve,execveat,read,readv,recvfrom,recvmsg,write,writev,sendto,sendmsg";
    fn run(work:&Path)->Result<()> {
            let mut restart=load_restart(work)?;
            if restart["old_frontend_stopped"]!=true || restart["state_preserved_after_old_exit"]!=true || restart["preservation_required"]!=true {return Err("RESTART_QUIESCENCE_NOT_PROVEN");}
            let old_pid=u32::try_from(restart["old_frontend_pid"].as_u64().ok_or("RESTART_OLD_PID")?).map_err(|_|"RESTART_OLD_PID")?;
            let expected_state=restart.get("after_stop_state").cloned().ok_or("RESTART_STATE_MISSING")?;
            if restart_state_identity()?!=expected_state{return Err("RESTART_STATE_CHANGED_BEFORE_START");}
            let observer_session=ProcessSession::read(std::process::id())?;
            let arm_deadline=Instant::now()+Duration::from_secs(6);
            let mut gated=prepare_gated_frontend(work,observer_session,old_pid,arm_deadline)?;
            let pid=gated.pid;let initial_session=gated.session;let baseline=tasks(pid)?;
            if baseline.len()!=1 || !baseline.contains_key(&pid) || baseline.values().any(|v|*v!=0){return Err("PREEXEC_BASELINE_NOT_PROVEN");}
            let recorder=Arc::new(Mutex::new(Recorder::new(pid)?));
            {
                let mut state=recorder.lock().map_err(|_|"RECORDER_LOCK")?;
                state.origins=KernelFdOrigins::new(&gated.initial_fds)?;
                state.baseline_threads=baseline.keys().copied().collect();state.allowed_threads.extend(baseline.keys().copied());
                state.startup_proof=Some(gated.projection());
                state.session_ownership=Some(json!({"observer_before_attach":observer_session.projection(),
                    "gated_shim_before_attach":initial_session.projection(),"frontend_before_canary":null,
                    "tracer_attached_before_canary":null,"frontend_after_detach":null,"observer_fresh_owned_session":true,
                    "tracer_inherits_owned_session":null,"frontend_ownership_preserved":null}));
            }
            let selectors=format!("trace={KERNEL_TRACE_SELECTORS}");
            let mut child=Command::new("/usr/bin/strace").args(["-f","-qq","-yy","-xx","-s","65536","-v","-e","signal=none",
                "-e","raw=execve,execveat","-e",&selectors,"-p",&pid.to_string()])
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_|"TRACE_SPAWN")?;
            let mut raw=child.stderr.take().ok_or("TRACE_PIPE")?;let stdout=child.stdout.take().ok_or("TRACE_PIPE")?;
            let discard=thread::spawn(move||std::io::copy(&mut std::io::BufReader::new(stdout),&mut std::io::sink()));
            let tracer=child.id();let mut trace=Trace{child,lifecycle:TraceLifecycle::default()};let shared=recorder.clone();
            let reader=thread::spawn(move||{
                let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
                    let mut chunk=[0;8192];let mut line=Vec::new();let mut oversized=false;let mut dropped=0u64;
                    loop {
                        let n=raw.read(&mut chunk).map_err(|_|"TRACE_PIPE_READ")?;
                        if n==0{break;}
                        for byte in &chunk[..n] {
                            if *byte==b'\n' {
                                if oversized {let mut state=shared.lock().map_err(|_|"RECORDER_LOCK")?;state.dropped+=dropped;state.fail("TRACE_LINE_LIMIT");}
                                else {
                                    let mut state=shared.lock().map_err(|_|"RECORDER_LOCK")?;
                                    match std::str::from_utf8(&line) {
                                        Ok(text)=>if let Err(id)=state.line(text){state.unparsed+=1;state.fail(id);state.note_shape(id,text);},
                                        Err(_)=>state.fail("TRACE_ENCODING"),
                                    }
                                }
                                line.clear();oversized=false;dropped=0;
                            } else if line.len()<LINE_LIMIT {line.push(*byte);}
                            else {oversized=true;dropped+=1;}
                        }
                    }
                    if !line.is_empty() || oversized {
                        let mut state=shared.lock().map_err(|_|"RECORDER_LOCK")?;
                        state.dropped+=dropped;state.fail("TRACE_PARTIAL_LINE");
                    }
                    Ok::<(),&'static str>(())
                }));
                if !matches!(result,Ok(Ok(()))) {if let Ok(mut state)=shared.lock(){state.fail("TRACE_READER_FAILED");}}
            });
            let mut attached=BTreeMap::new();let mut released=false;let mut armed=false;let mut native=false;let mut coverage_deadline=None;
            let deadline=Instant::now()+Duration::from_secs(90);
            let result=(||->Result<()> {
                loop {
                    let current=tasks(pid)?;
                    if !released {
                        if current.len()==1 && current.get(&pid)==Some(&tracer) {
                            if ProcessSession::read(pid)?!=initial_session{return Err("GATED_SESSION_CHANGED_BEFORE_RELEASE");}
                            let tracer_session=ProcessSession::read(tracer)?;
                            if !tracer_session.inherits(observer_session){return Err("TRACER_SESSION_NOT_INHERITED");}
                            recorder.lock().map_err(|_|"RECORDER_LOCK")?.session_ownership.as_mut().ok_or("SESSION_PROOF_MISSING")?["tracer_attached_before_canary"]=tracer_session.projection();
                            recorder.lock().map_err(|_|"RECORDER_LOCK")?.session_ownership.as_mut().ok_or("SESSION_PROOF_MISSING")?["tracer_inherits_owned_session"]=json!(true);
                            restart["release_has_occurred"]=json!(true);save_restart(work,&restart)?;
                            gated.release()?;released=true;
                            recorder.lock().map_err(|_|"RECORDER_LOCK")?.startup_proof=Some(gated.projection());
                        }
                    }else if current.values().any(|v|*v!=tracer) {
                        let snapshot=thread_coverage_loss_snapshot(pid,&baseline,&current,tracer);
                        recorder.lock().map_err(|_|"RECORDER_LOCK")?.thread_coverage_loss=Some(snapshot);
                        return Err("THREAD_COVERAGE_LOST");
                    }
                    if released && !armed {
                        let ready={let state=recorder.lock().map_err(|_|"RECORDER_LOCK")?;state.exec_identity.is_some() && state.origins.listener_ready() && state.failures.is_empty()};
                        if ready && current.values().all(|v|*v==tracer) {
                            let identity=verified_frontend_identity(pid)?;
                            if ProcessSession::read(pid)?!=initial_session || identity["ppid"]!=observer_session.pid || gated.child.try_wait().map_err(|_|"GATED_CHILD_WAIT")?.is_some(){return Err("NEW_FRONTEND_OWNERSHIP_CHANGED");}
                            let actual_state=restart_state_identity()?;
                            if actual_state!=expected_state{return Err("RESTART_STATE_CHANGED_AFTER_START");}
                            let before_canary=tasks(pid)?;
                            if before_canary.is_empty() || before_canary.values().any(|v|*v!=tracer){return Err("STARTUP_THREAD_COVERAGE_CHANGED");}
                            let mut state=recorder.lock().map_err(|_|"RECORDER_LOCK")?;
                            if !state.failures.is_empty() || !state.origins.listener_ready() {return Err("STARTUP_OBSERVER_CHANGED");}
                            if state.exec_identity.as_ref()!=Some(&identity){return Err("NEW_FRONTEND_IDENTITY_CHANGED");}
                            restart["after_start_state"]=actual_state;restart["new_frontend_identity"]=identity.clone();
                            restart["new_frontend_identity_and_state_proven"]=json!(true);restart["preservation_required"]=json!(false);save_restart(work,&restart)?;
                            state.startup_identity_and_state_proven=true;state.restart_preservation_required=false;state.attachment_proven=true;
                            state.session_ownership.as_mut().ok_or("SESSION_PROOF_MISSING")?["frontend_before_canary"]=identity;
                            attached=before_canary;armed=true;save(work,&state.projection(true,false,&attached,tracer))?;
                        }
                    }
                    if !armed && Instant::now()>arm_deadline{return Err("STARTUP_KERNEL_ORIGIN_TIMEOUT");}
                    let command=if work.join("native-observer-command").exists(){bounded_file(&work.join("native-observer-command"),16)?}else{Vec::new()};
                    if command==b"stop"{break;}
                    if armed && !native && command==b"native" {
                        let limit=coverage_deadline.get_or_insert_with(||Instant::now()+Duration::from_secs(3));
                        let mut state=recorder.lock().map_err(|_|"RECORDER_LOCK")?;
                        if state.canary_ok(){state.native()?;native=true;save(work,&state.projection(true,false,&current,tracer))?;}
                        else if Instant::now()>=*limit{return Err("CANARY_COVERAGE");}
                    }
                    if !recorder.lock().map_err(|_|"RECORDER_LOCK")?.failures.is_empty(){return Err("TRACE_DECODE_FAILED");}
                    if trace.try_wait(WAIT_OBSERVE).map_err(|_|"TRACE_WAIT")?.is_some(){return Err("TRACE_EXITED");}
                    if gated.child.try_wait().map_err(|_|"GATED_CHILD_WAIT")?.is_some(){return Err("NEW_FRONTEND_EXITED");}
                    if Instant::now()>deadline{return Err("OBSERVATION_DEADLINE");}
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(())
            })();
            if let Err(id)=result {recorder.lock().map_err(|_|"RECORDER_LOCK")?.fail(id);}
            if !released {let _=gated.rollback_before_release();}
            if let Err(id)=trace.stop(){recorder.lock().map_err(|_|"RECORDER_LOCK")?.fail(id);}
            if reader.join().is_err(){recorder.lock().map_err(|_|"RECORDER_LOCK")?.fail("TRACE_READER_FAILED");}
            match discard.join() {
                Ok(Ok(0))=>{},Ok(Ok(n))=>{let mut state=recorder.lock().map_err(|_|"RECORDER_LOCK")?;state.dropped+=n;state.fail("UNEXPECTED_TRACE_STDOUT");},
                _=>recorder.lock().map_err(|_|"RECORDER_LOCK")?.fail("TRACE_STDOUT_FAILED"),
            }
            let after_identity=verified_frontend_identity(pid);
            let disarmed=after_identity.is_ok() && tasks(pid).is_ok_and(|v|v.values().all(|v|*v==0));
            let after_state=if disarmed {restart_state_identity()}else{Err("DETACH_NOT_PROVEN")};
            let mut state=recorder.lock().map_err(|_|"RECORDER_LOCK")?;
            state.startup_proof=Some(gated.projection());
            if !disarmed{state.fail("DETACH_NOT_PROVEN");state.restart_preservation_required=true;}
            let identity_preserved=after_identity.as_ref().is_ok_and(|v|state.exec_identity.as_ref()==Some(v) && v["ppid"]==observer_session.pid) &&
                ProcessSession::read(pid).is_ok_and(|v|v==initial_session) && gated.child.try_wait().is_ok_and(|v|v.is_none());
            if let Some(proof)=state.session_ownership.as_mut(){proof["frontend_after_detach"]=after_identity.as_ref().ok().cloned().unwrap_or(Value::Null);proof["frontend_ownership_preserved"]=json!(identity_preserved);}
            if !identity_preserved{state.fail("NEW_FRONTEND_SURVIVAL_IDENTITY_NOT_PROVEN");state.restart_preservation_required=true;state.startup_identity_and_state_proven=false;}
            match after_state {
                Ok(value) if value==expected_state=>{restart["after_detach_state"]=value;restart["state_preserved_after_detach"]=json!(true);},
                _=>{state.fail("RESTART_STATE_CHANGED_OR_UNKNOWN_AFTER_DETACH");state.restart_preservation_required=true;state.startup_identity_and_state_proven=false;},
            }
            if state.origins.pending.values().any(|entry|matches!(entry.call,KernelCall::Write|KernelCall::Writev|KernelCall::Sendto|KernelCall::Sendmsg) &&
                entry.inputs[..entry.input_count].iter().any(|input|state.origins.sockets.get(&input.socket_generation).is_some_and(|v|v.state==KernelSocketState::AcceptedTarget))) {state.fail("PENDING_WRITE");}
            restart["preservation_required"]=json!(state.restart_preservation_required || !state.startup_identity_and_state_proven);
            restart["new_frontend_after_detach_identity"]=after_identity.ok().unwrap_or(Value::Null);ledger_preserve_child_failure(work,&mut restart)?;save_restart(work,&restart)?;
            state.tracer_lifecycle=Some(trace.lifecycle.projection());let projection=state.projection(false,disarmed,&attached,tracer);save(work,&projection)?;
            if projection["capture_complete"]==true {Ok(())}else{Err("CAPTURE_INCOMPLETE")}
        }
    pub fn dispatch()->Result<()> {
        let args=std::env::args_os().skip(1).collect::<Vec<_>>();
        if args.len()==1 && args[0]=="--observe-ledger-snapshot" {return restart_ledger_stdio();}
        let work=Path::new(args.get(1).ok_or("ARGS")?);
        if args.first().is_some_and(|v|v=="--observe-quiesce") && args.len()==3 {
            let pid=args[2].to_str().and_then(|v|v.parse::<u32>().ok()).filter(|v|*v>0 && *v<=i32::MAX as u32).ok_or("ARGS")?;
            return quiesce_owned_frontend(work,pid);
        }
        if args.first().is_some_and(|v|v=="--observe") && args.len()==2 {
            let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||run(work))).unwrap_or(Err("OBSERVER_PANIC"));
            if let Err(id)=result {
                let mut value=bounded_file(&status_path(work),LIMIT*8).ok().and_then(|v|serde_json::from_slice::<Value>(&v).ok()).unwrap_or_else(||json!({}));
                value["armed"]=json!(false);value["capture_complete"]=json!(false);
                let preserve=value["startup_identity_and_state_proven"]!=true || value["restart_preservation_required"]==true;
                if preserve {value["restart_preservation_required"]=json!(true);}
                let ids=value.as_object_mut().ok_or("STATUS_SHAPE")?.entry("failure_ids").or_insert_with(||json!([]));
                if let Some(ids)=ids.as_array_mut(){if !ids.iter().any(|v|v==id){ids.push(json!(id));}}
                let _=save(work,&value);
            }
            return result;
        }
        if args.len()!=3 || args[0]!="--observe-control"{return Err("ARGS");}
        let action=args[2].to_str().ok_or("ARGS")?;
        if matches!(action,"native"|"stop"){fs::write(work.join("native-observer-command"),action).map_err(|_|"COMMAND_WRITE")?;}
        else if action!="armed"{return Err("ARGS");}
        let deadline=Instant::now()+Duration::from_secs(8);
        loop {
            if let Ok(bytes)=bounded_file(&status_path(work),LIMIT*8){if let Ok(v)=serde_json::from_slice::<Value>(&bytes){
                if action=="stop" && v["disarmed"]==true{return Ok(());}
                if v["failure_ids"].as_array().is_some_and(|v|!v.is_empty()){return Err("OBSERVER_FAILED");}
                let attached=v["full_thread_attachment_proven_before_canary_and_native"]==true;
                if action=="armed" && v["armed"]==true && attached && v["startup_identity_and_state_proven"]==true{return Ok(());}
                if action=="native" && v["phase"]=="native" && v["armed"]==true && attached && !v["canary"].is_null(){return Ok(());}
            }}
            if Instant::now()>=deadline{return Err("CONTROL_DEADLINE");}thread::sleep(Duration::from_millis(20));
        }
    }
}
