use crate::decode::Decode;
use crate::error::Error;
use crate::mssql::protocol::done::Status;
use crate::mssql::protocol::message::Message;
use crate::mssql::protocol::packet::PacketType;
use crate::mssql::protocol::rpc::{OptionFlags, Procedure, RpcRequest};
use crate::mssql::statement::MssqlStatementMetadata;
use crate::mssql::{Mssql, MssqlArguments, MssqlConnection, MssqlTypeInfo, MssqlValueRef};
use either::Either;
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;

pub(crate) async fn prepare(
    conn: &mut MssqlConnection,
    sql: &str,
) -> Result<Arc<MssqlStatementMetadata>, Error> {
    if let Some(metadata) = conn.cache_statement.get_mut(sql) {
        return Ok(metadata.clone());
    }

    // NOTE: this does not support unicode identifiers; as we don't even support
    //       named parameters (yet) this is probably fine, for now

    static PARAMS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"@p[[:alnum:]]+").unwrap());

    let mut params = String::new();

    for m in PARAMS_RE.captures_iter(sql) {
        if !params.is_empty() {
            params.push_str(",");
        }

        params.push_str(&m[0]);

        // NOTE: this means that a query! of `SELECT @p1` will have the macros believe
        //       it will return nvarchar(1); this is a greater issue with `query!` that we
        //       we need to circle back to. This doesn't happen much in practice however.
        params.push_str(" nvarchar(1)");
    }

    let params = if params.is_empty() {
        None
    } else {
        Some(&*params)
    };

    let mut args = MssqlArguments::default();

    args.declare("", 0_i32);
    args.add_unnamed(params);
    args.add_unnamed(sql);
    args.add_unnamed(0x0001_i32); // 1 = SEND_METADATA

    conn.stream.write_packet(
        PacketType::Rpc,
        RpcRequest {
            transaction_descriptor: conn.stream.transaction_descriptor,
            arguments: &args,
            // [sp_prepare] will emit the column meta data
            // small issue is that we need to declare all the used placeholders with a "fallback" type
            // we currently use regex to collect them; false positives are *okay* but false
            // negatives would break the query
            procedure: Either::Right(Procedure::Prepare),
            options: OptionFlags::empty(),
        },
    );

    conn.stream.flush().await?;
    conn.stream.wait_until_ready().await?;
    conn.stream.pending_done_count += 1;

    let mut id: Option<i32> = None;

    loop {
        let message = conn.stream.recv_message().await?;

        match message {
            Message::DoneProc(done) | Message::Done(done) => {
                if !done.status.contains(Status::DONE_MORE) {
                    // done with prepare
                    conn.stream.handle_done(&done);
                    break;
                }
            }

            Message::ReturnValue(rv) => {
                id = <i32 as Decode<Mssql>>::decode(MssqlValueRef {
                    data: rv.value.as_ref(),
                    type_info: MssqlTypeInfo(rv.type_info),
                })
                .ok();
            }

            _ => {}
        }
    }

    if let Some(id) = id {
        let mut args = MssqlArguments::default();
        args.add_unnamed(id);

        conn.stream.write_packet(
            PacketType::Rpc,
            RpcRequest {
                transaction_descriptor: conn.stream.transaction_descriptor,
                arguments: &args,
                procedure: Either::Right(Procedure::Unprepare),
                options: OptionFlags::empty(),
            },
        );

        conn.stream.flush().await?;
        conn.stream.wait_until_ready().await?;
        conn.stream.pending_done_count += 1;

        loop {
            let message = conn.stream.recv_message().await?;

            match message {
                Message::DoneProc(done) | Message::Done(done) => {
                    if !done.status.contains(Status::DONE_MORE) {
                        // done with unprepare
                        conn.stream.handle_done(&done);
                        break;
                    }
                }

                _ => {}
            }
        }
    }

    let metadata = Arc::new(MssqlStatementMetadata {
        columns: conn.stream.columns.as_ref().clone(),
        column_names: conn.stream.column_names.as_ref().clone(),
    });

    conn.cache_statement.insert(sql, metadata.clone());

    Ok(metadata)
}
