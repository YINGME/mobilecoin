// Copyright (c) 2018-2020 MobileCoin Inc.

//! JSON wrapper for the mobilecoind API.

#![feature(proc_macro_hygiene, decl_macro)]

use grpcio::ChannelBuilder;
use mc_api::external::{CompressedRistretto, KeyImage, PublicAddress};
use mc_common::logger::{create_app_logger, log, o};
use mc_mobilecoind_api::{mobilecoind_api_grpc::MobilecoindApiClient, MobilecoindUri};
use mc_mobilecoind_json::data_types::*;
use mc_util_grpc::ConnectionUriGrpcioChannel;
use protobuf::RepeatedField;
use rocket::{get, post, routes};
use rocket_contrib::json::Json;
use std::{convert::TryFrom, sync::Arc};
use structopt::StructOpt;

/// Command line config, set with defaults that will work with
/// a standard mobilecoind instance
#[derive(Clone, Debug, StructOpt)]
#[structopt(name = "mobilecoind-json", about = "A REST frontend for mobilecoind")]
pub struct Config {
    /// Host to listen on.
    #[structopt(long, default_value = "127.0.0.1")]
    pub listen_host: String,

    /// Port to start webserver on.
    #[structopt(long, default_value = "9090")]
    pub listen_port: u16,

    /// MobileCoinD URI.
    #[structopt(long, default_value = "insecure-mobilecoind://127.0.0.1/")]
    pub mobilecoind_uri: MobilecoindUri,
}

/// Connection to the mobilecoind client
struct State {
    pub mobilecoind_api_client: MobilecoindApiClient,
}

/// Requests a new root entropy from mobilecoind
#[get("/entropy")]
fn entropy(state: rocket::State<State>) -> Result<Json<JsonEntropyResponse>, String> {
    let resp = state
        .mobilecoind_api_client
        .generate_entropy(&mc_mobilecoind_api::Empty::new())
        .map_err(|err| format!("Failed getting entropy: {}", err))?;
    Ok(Json(JsonEntropyResponse::from(&resp)))
}

/// Creates a monitor. Data for the key and range is POSTed using the struct above.
#[post("/monitors", format = "json", data = "<monitor>")]
fn create_monitor(
    state: rocket::State<State>,
    monitor: Json<JsonMonitorRequest>,
) -> Result<Json<JsonMonitorResponse>, String> {
    let entropy = hex::decode(&monitor.entropy)
        .map_err(|err| format!("Failed to decode hex key: {}", err))?;

    let mut req = mc_mobilecoind_api::GetAccountKeyRequest::new();
    req.set_entropy(entropy.to_vec());

    let mut resp = state
        .mobilecoind_api_client
        .get_account_key(&req)
        .map_err(|err| format!("Failed getting account key for entropy: {}", err))?;

    let account_key = resp.take_account_key();

    let mut req = mc_mobilecoind_api::AddMonitorRequest::new();
    req.set_account_key(account_key);
    req.set_first_subaddress(monitor.first_subaddress);
    req.set_num_subaddresses(monitor.num_subaddresses);
    req.set_first_block(0);

    let monitor_response = state
        .mobilecoind_api_client
        .add_monitor(&req)
        .map_err(|err| format!("Failed adding monitor: {}", err))?;

    Ok(Json(JsonMonitorResponse::from(&monitor_response)))
}

/// Gets a list of existing monitors
#[get("/monitors")]
fn monitors(state: rocket::State<State>) -> Result<Json<JsonMonitorListResponse>, String> {
    let resp = state
        .mobilecoind_api_client
        .get_monitor_list(&mc_mobilecoind_api::Empty::new())
        .map_err(|err| format!("Failed getting monitor list: {}", err))?;
    Ok(Json(JsonMonitorListResponse::from(&resp)))
}

/// Get the current status of a created monitor
#[get("/monitors/<monitor_hex>")]
fn monitor_status(
    state: rocket::State<State>,
    monitor_hex: String,
) -> Result<Json<JsonMonitorStatusResponse>, String> {
    let monitor_id =
        hex::decode(monitor_hex).map_err(|err| format!("Failed to decode monitor hex: {}", err))?;

    let mut req = mc_mobilecoind_api::GetMonitorStatusRequest::new();
    req.set_monitor_id(monitor_id);

    let resp = state
        .mobilecoind_api_client
        .get_monitor_status(&req)
        .map_err(|err| format!("Failed getting monitor status: {}", err))?;

    Ok(Json(JsonMonitorStatusResponse::from(&resp)))
}

/// Balance check using a created monitor and subaddress index
#[get("/monitors/<monitor_hex>/<subaddress_index>/balance")]
fn balance(
    state: rocket::State<State>,
    monitor_hex: String,
    subaddress_index: u64,
) -> Result<Json<JsonBalanceResponse>, String> {
    let monitor_id =
        hex::decode(monitor_hex).map_err(|err| format!("Failed to decode monitor hex: {}", err))?;

    let mut req = mc_mobilecoind_api::GetBalanceRequest::new();
    req.set_monitor_id(monitor_id);
    req.set_subaddress_index(subaddress_index);

    let resp = state
        .mobilecoind_api_client
        .get_balance(&req)
        .map_err(|err| format!("Failed getting balance: {}", err))?;

    Ok(Json(JsonBalanceResponse::from(&resp)))
}

/// Generates a request code with an optional value and memo
#[post(
    "/monitors/<monitor_hex>/<subaddress_index>/request-code",
    format = "json",
    data = "<extra>"
)]
fn request_code(
    state: rocket::State<State>,
    monitor_hex: String,
    subaddress_index: u64,
    extra: Json<JsonRequestCodeRequest>,
) -> Result<Json<JsonRequestCodeResponse>, String> {
    let monitor_id =
        hex::decode(monitor_hex).map_err(|err| format!("Failed to decode monitor hex: {}", err))?;

    // Get our public address.
    let mut req = mc_mobilecoind_api::GetPublicAddressRequest::new();
    req.set_monitor_id(monitor_id);
    req.set_subaddress_index(subaddress_index);

    let resp = state
        .mobilecoind_api_client
        .get_public_address(&req)
        .map_err(|err| format!("Failed getting public address: {}", err))?;

    let public_address = resp.get_public_address().clone();

    // Generate b58 code
    let mut req = mc_mobilecoind_api::GetRequestCodeRequest::new();
    req.set_receiver(public_address);
    if let Some(value) = extra.value {
        req.set_value(value);
    }
    if let Some(memo) = extra.memo.clone() {
        req.set_memo(memo);
    }

    let resp = state
        .mobilecoind_api_client
        .get_request_code(&req)
        .map_err(|err| format!("Failed getting request code: {}", err))?;

    Ok(Json(JsonRequestCodeResponse::from(&resp)))
}

/// Retrieves the data in a request code
#[get("/read-request/<request_code>")]
fn read_request(
    state: rocket::State<State>,
    request_code: String,
) -> Result<Json<JsonReadRequestResponse>, String> {
    let mut req = mc_mobilecoind_api::ReadRequestCodeRequest::new();
    req.set_b58_code(request_code);
    let resp = state
        .mobilecoind_api_client
        .read_request_code(&req)
        .map_err(|err| format!("Failed reading request code: {}", err))?;

    // The response contains the public keys encoded in the read request, as well as a memo and
    // requested value. This can be used as-is in the transfer call below, or the value can be
    // modified.
    Ok(Json(JsonReadRequestResponse::from(&resp)))
}

/// Performs a transfer from a monitor and subaddress. The public keys and amount are in the POST data.
#[post(
    "/monitors/<monitor_hex>/<subaddress_index>/transfer",
    format = "json",
    data = "<transfer>"
)]
fn transfer(
    state: rocket::State<State>,
    monitor_hex: String,
    subaddress_index: u64,
    transfer: Json<JsonReadRequestResponse>,
) -> Result<Json<JsonTransferResponse>, String> {
    let monitor_id =
        hex::decode(monitor_hex).map_err(|err| format!("Failed to decode monitor hex: {}", err))?;

    let public_address = PublicAddress::try_from(&transfer.receiver)?;

    // Generate an outlay
    let mut outlay = mc_mobilecoind_api::Outlay::new();
    outlay.set_receiver(public_address);
    outlay.set_value(
        transfer
            .value
            .parse::<u64>()
            .map_err(|err| format!("Failed to parse amount: {}", err))?,
    );

    // Send the payment request
    let mut req = mc_mobilecoind_api::SendPaymentRequest::new();
    req.set_sender_monitor_id(monitor_id);
    req.set_sender_subaddress(subaddress_index);
    req.set_outlay_list(RepeatedField::from_vec(vec![outlay]));

    let resp = state
        .mobilecoind_api_client
        .send_payment(&req)
        .map_err(|err| format!("Failed to send payment: {}", err))?;

    // The receipt from the payment request can be used by the status check below
    Ok(Json(JsonTransferResponse::from(&resp)))
}

/// Checks the status of a transfer given a key image and tombstone block
#[post("/check-transfer-status", format = "json", data = "<receipt>")]
fn check_transfer_status(
    state: rocket::State<State>,
    receipt: Json<JsonTransferResponse>,
) -> Result<Json<JsonStatusResponse>, String> {
    let mut sender_receipt = mc_mobilecoind_api::SenderTxReceipt::new();
    let mut key_images = Vec::new();
    for key_image_hex in &receipt.sender_tx_receipt.key_images {
        key_images.push(KeyImage::from(
            hex::decode(&key_image_hex).map_err(|err| format!("{}", err))?,
        ))
    }

    sender_receipt.set_key_image_list(RepeatedField::from_vec(key_images));
    sender_receipt.set_tombstone(receipt.sender_tx_receipt.tombstone);

    let mut req = mc_mobilecoind_api::GetTxStatusAsSenderRequest::new();
    req.set_receipt(sender_receipt);

    let resp = state
        .mobilecoind_api_client
        .get_tx_status_as_sender(&req)
        .map_err(|err| format!("Failed getting status: {}", err))?;

    Ok(Json(JsonStatusResponse::from(&resp)))
}

/// Checks the status of a transfer given data for a specific receiver
/// The sender of the transaction will take specific receipt data from the /transfer call
/// and distribute it to the recipient(s) so they can verify that a transaction has been
/// processed and the the person supplying the receipt can prove they intiated it
#[post("/check-receiver-transfer-status", format = "json", data = "<receipt>")]
fn check_receiver_transfer_status(
    state: rocket::State<State>,
    receipt: Json<JsonReceiverTxReceipt>,
) -> Result<Json<JsonStatusResponse>, String> {
    let mut receiver_receipt = mc_mobilecoind_api::ReceiverTxReceipt::new();
    let mut tx_public_key = CompressedRistretto::new();
    tx_public_key.set_data(hex::decode(&receipt.tx_public_key).map_err(|err| format!("{}", err))?);
    receiver_receipt.set_tx_public_key(tx_public_key);
    receiver_receipt
        .set_tx_out_hash(hex::decode(&receipt.tx_out_hash).map_err(|err| format!("{}", err))?);
    receiver_receipt.set_tombstone(receipt.tombstone);
    receiver_receipt.set_confirmation_number(
        hex::decode(&receipt.confirmation_number).map_err(|err| format!("{}", err))?,
    );

    let mut req = mc_mobilecoind_api::GetTxStatusAsReceiverRequest::new();
    req.set_receipt(receiver_receipt);

    let resp = state
        .mobilecoind_api_client
        .get_tx_status_as_receiver(&req)
        .map_err(|err| format!("Failed getting status: {}", err))?;

    Ok(Json(JsonStatusResponse::from(&resp)))
}

/// Gets information about the entire ledger
#[get("/ledger-info")]
fn ledger_info(state: rocket::State<State>) -> Result<Json<JsonLedgerInfoResponse>, String> {
    let resp = state
        .mobilecoind_api_client
        .get_ledger_info(&mc_mobilecoind_api::Empty::new())
        .map_err(|err| format!("Failed getting ledger info: {}", err))?;

    Ok(Json(JsonLedgerInfoResponse::from(&resp)))
}

/// Retrieves the data in a request code
#[get("/block-info/<block_num>")]
fn block_info(
    state: rocket::State<State>,
    block_num: u64,
) -> Result<Json<JsonBlockInfoResponse>, String> {
    let mut req = mc_mobilecoind_api::GetBlockInfoRequest::new();
    req.set_block(block_num);

    let resp = state
        .mobilecoind_api_client
        .get_block_info(&req)
        .map_err(|err| format!("Failed getting ledger info: {}", err))?;

    Ok(Json(JsonBlockInfoResponse::from(&resp)))
}

/// Retrieves the details for a given block.
#[get("/block-details/<block_num>")]
fn block_details(
    state: rocket::State<State>,
    block_num: u64,
) -> Result<Json<JsonBlockDetailsResponse>, String> {
    let mut req = mc_mobilecoind_api::GetBlockRequest::new();
    req.set_block(block_num);

    let resp = state
        .mobilecoind_api_client
        .get_block(&req)
        .map_err(|err| format!("Failed getting block details: {}", err))?;

    Ok(Json(JsonBlockDetailsResponse::from(&resp)))
}
/// Retreives processed block information.
#[get("/processed-block/<monitor_hex>/<block_num>")]
fn processed_block(
    state: rocket::State<State>,
    monitor_hex: String,
    block_num: u64,
) -> Result<Json<JsonProcessedBlockResponse>, String> {
    let monitor_id =
        hex::decode(monitor_hex).map_err(|err| format!("Failed to decode monitor hex: {}", err))?;

    let mut req = mc_mobilecoind_api::GetProcessedBlockRequest::new();
    req.set_monitor_id(monitor_id);
    req.set_block(block_num);

    let resp = state
        .mobilecoind_api_client
        .get_processed_block(&req)
        .map_err(|err| format!("Failed getting processed block: {}", err))?;

    Ok(Json(JsonProcessedBlockResponse::from(&resp)))
}

/// Generates an AddressRequest code with a URL for the client to POST payment instructions
#[post("/address-request", format = "json", data = "<address_request>")]
fn address_request_code(
    state: rocket::State<State>,
    address_request: Json<JsonAddressRequestCodeRequest>,
) -> Result<Json<JsonAddressRequestCodeResponse>, String> {
    let mut req = mc_mobilecoind_api::GetAddressRequestCodeRequest::new();
    req.set_url(address_request.url.clone());

    let resp = state
        .mobilecoind_api_client
        .get_address_request_code(&req)
        .map_err(|err| format!("Failed address code: {}", err))?;

    Ok(Json(JsonAddressRequestCodeResponse::from(&resp)))
}

fn main() {
    mc_common::setup_panic_handler();
    let _sentry_guard = mc_common::sentry::init();

    let config = Config::from_args();

    let (logger, _global_logger_guard) = create_app_logger(o!());
    log::info!(
        logger,
        "Starting mobilecoind HTTP gateway on {}:{}, connecting to {}",
        config.listen_host,
        config.listen_port,
        config.mobilecoind_uri,
    );

    // Set up the gRPC connection to the mobilecoind client
    let env = Arc::new(grpcio::EnvBuilder::new().build());
    let ch = ChannelBuilder::new(env)
        .max_receive_message_len(std::i32::MAX)
        .max_send_message_len(std::i32::MAX)
        .connect_to_uri(&config.mobilecoind_uri, &logger);

    let mobilecoind_api_client = MobilecoindApiClient::new(ch);

    let rocket_config = rocket::Config::build(rocket::config::Environment::Production)
        .address(&config.listen_host)
        .port(config.listen_port)
        .unwrap();

    rocket::custom(rocket_config)
        .mount(
            "/",
            routes![
                entropy,
                create_monitor,
                monitors,
                monitor_status,
                balance,
                request_code,
                read_request,
                transfer,
                check_transfer_status,
                check_receiver_transfer_status,
                ledger_info,
                block_info,
                block_details,
                processed_block,
                address_request_code,
            ],
        )
        .manage(State {
            mobilecoind_api_client,
        })
        .launch();
}
