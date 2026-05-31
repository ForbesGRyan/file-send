//! RtcPeerConnection / data channel setup and SDP/ICE plumbing.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    RtcConfiguration, RtcDataChannel, RtcIceServer, RtcPeerConnection,
    RtcPeerConnectionIceEvent, RtcSdpType, RtcSessionDescriptionInit,
};

/// Default public STUN server; override at build time via the `STUN_URL` env
/// (read by `option_env!` so it bakes into the wasm at compile time).
pub fn stun_url() -> String {
    option_env!("STUN_URL")
        .unwrap_or("stun:stun.l.google.com:19302")
        .to_string()
}

/// Create a peer connection configured with the STUN server.
pub fn new_peer_connection() -> Result<RtcPeerConnection, JsValue> {
    let ice_server = RtcIceServer::new();
    // `urls` accepts a string or array; a single string is fine.
    ice_server.set_urls(&JsValue::from_str(&stun_url()));

    let ice_servers = js_sys::Array::new();
    ice_servers.push(&ice_server);

    let config = RtcConfiguration::new();
    config.set_ice_servers(&ice_servers);

    RtcPeerConnection::new_with_configuration(&config)
}

/// Forward locally-gathered ICE candidates to a sink (the signaling sender).
/// `on_candidate` receives the JSON-encoded candidate init, ready to wrap in
/// `SignalMsg::Ice`.
pub fn on_ice_candidate(pc: &RtcPeerConnection, on_candidate: impl Fn(String) + 'static) {
    let cb = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(
        move |ev: RtcPeerConnectionIceEvent| {
            if let Some(candidate) = ev.candidate() {
                // Serialize the candidate to JSON via JS so the remote can
                // reconstruct an RtcIceCandidateInit.
                let obj = candidate.to_json();
                if let Ok(json) = js_sys::JSON::stringify(&obj) {
                    if let Some(s) = json.as_string() {
                        on_candidate(s);
                    }
                }
            }
        },
    );
    pc.set_onicecandidate(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}

/// Create an offer, set it as local description, return the SDP string.
pub async fn create_offer(pc: &RtcPeerConnection) -> Result<String, JsValue> {
    let offer = JsFuture::from(pc.create_offer()).await?;
    let sdp = js_sys::Reflect::get(&offer, &JsValue::from_str("sdp"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("offer missing sdp"))?;
    set_local(pc, RtcSdpType::Offer, &sdp).await?;
    Ok(sdp)
}

/// Create an answer (after a remote offer is set), return the SDP string.
pub async fn create_answer(pc: &RtcPeerConnection) -> Result<String, JsValue> {
    let answer = JsFuture::from(pc.create_answer()).await?;
    let sdp = js_sys::Reflect::get(&answer, &JsValue::from_str("sdp"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("answer missing sdp"))?;
    set_local(pc, RtcSdpType::Answer, &sdp).await?;
    Ok(sdp)
}

async fn set_local(pc: &RtcPeerConnection, kind: RtcSdpType, sdp: &str) -> Result<(), JsValue> {
    let desc = RtcSessionDescriptionInit::new(kind);
    desc.set_sdp(sdp);
    JsFuture::from(pc.set_local_description(&desc)).await?;
    Ok(())
}

/// Apply a remote SDP description (offer or answer).
pub async fn set_remote(pc: &RtcPeerConnection, kind: RtcSdpType, sdp: &str) -> Result<(), JsValue> {
    let desc = RtcSessionDescriptionInit::new(kind);
    desc.set_sdp(sdp);
    JsFuture::from(pc.set_remote_description(&desc)).await?;
    Ok(())
}

/// Add a remote ICE candidate from its JSON-encoded init string.
pub async fn add_ice_candidate(pc: &RtcPeerConnection, candidate_json: &str) -> Result<(), JsValue> {
    let obj = js_sys::JSON::parse(candidate_json)?;
    let init = obj.unchecked_into::<web_sys::RtcIceCandidateInit>();
    let candidate = web_sys::RtcIceCandidate::new(&init)?;
    JsFuture::from(pc.add_ice_candidate_with_opt_rtc_ice_candidate(Some(&candidate))).await?;
    Ok(())
}

/// Create an ordered+reliable data channel with the given `label`.
///
/// The first channel (the control channel) bootstraps the SCTP transport via
/// the initial SDP negotiation. Channels created afterwards — one per file —
/// are multiplexed over that same transport and need no renegotiation.
pub fn create_data_channel(pc: &RtcPeerConnection, label: &str) -> RtcDataChannel {
    // Default config is ordered + reliable, which is what we want.
    pc.create_data_channel(label)
}

/// Register a handler for the data channel created by the remote (joiner side).
pub fn on_data_channel(pc: &RtcPeerConnection, on_dc: impl Fn(RtcDataChannel) + 'static) {
    let cb = Closure::<dyn FnMut(web_sys::RtcDataChannelEvent)>::new(
        move |ev: web_sys::RtcDataChannelEvent| {
            on_dc(ev.channel());
        },
    );
    pc.set_ondatachannel(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}
