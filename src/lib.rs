//! VIZIO SmartCast TVs, over the local HTTPS API built into the set, port 7345 (older
//! firmware: 9000). Self-signed certificate, wrong CN — core accepts it for a bare IP the same
//! way it accepts every other LAN bridge's; see `Tls::LOCAL` in `core`.
//!
//! ```text
//!   PUT  /pairing/start   {DEVICE_ID,DEVICE_NAME}                unauthenticated, shows a PIN
//!   PUT  /pairing/pair    {DEVICE_ID,CHALLENGE_TYPE,
//!                          PAIRING_REQ_TOKEN,RESPONSE_VALUE}     unauthenticated, returns AUTH_TOKEN
//!   PUT  /key_command/    {KEYLIST:[{CODESET,CODE,ACTION}]}      header AUTH: <token>
//!   GET  /state/device/power_mode                                header AUTH: <token>
//!   GET  /menu_native/dynamic/tv_settings/devices/current_input  header AUTH: <token>
//!   PUT  /menu_native/dynamic/tv_settings/devices/current_input  {REQUEST,VALUE,HASHVAL}
//! ```
//!
//! Every reply is `{"STATUS":{...}, ...}`, and nothing in `on_event`'s arguments says which
//! request it answers — the http layer does not carry that across, same as Roku's. So replies
//! are told apart by shape, the same choice `hisense` makes for its MQTT messages: a bare
//! number in `ITEMS[0].VALUE` is a power reading, a string is the current input's name. A
//! `key_command` reply carries no `ITEMS` at all — ordinarily nothing to read there, except
//! mid-seek (see below), where its absence is itself the signal.
//!
//! # `set_input` writes the CNAME, not the name
//!
//! `PUT .../current_input {REQUEST:"MODIFY",VALUE:<input>,HASHVAL:<hash>}` takes the input's
//! **`CNAME`** — `hdmi2`, lowercase, no hyphen — and answers `FAILURE` for its display `NAME`
//! (`HDMI-2`), which is what the reference Python client (`pyvizio`) sends and what every
//! write here did until it was checked against real hardware. The two are easy to confuse
//! because `.../devices/name_input` reports both, and the failure looks exactly like the
//! open "cannot change input" firmware bug that client's users report — silent, with a
//! correctly-formed request. Verified against a real set: `hdmi2` switches it, `HDMI-2` does
//! not, and nothing else about the request differs.
//!
//! So there are three names per input and each has one job. `CNAME` (`hdmi2`) is what a write
//! takes. `NAME` (`HDMI-2`) is what `current_input` reads back, so it is what a reply is
//! matched on. `CUSTOM_NAME` (`Videocore` on a set where somebody renamed that port) is
//! cosmetic and matched on nothing — it moves under a person and would stop matching.
//!
//! # The inputs are read, not assumed
//!
//! A manifest declares four HDMI ports because it describes a product line; the set this was
//! written against has three, and the fourth is a jack the pathfinder would happily route a
//! room through. So `current_inputs` is read on every bind and answered with
//! [`HostCall::Connections`], which replaces that guess for this one unit.
//!
//! Connection ids are **derived from the `CNAME`** — `hdmi2` is always 1002 — rather than from
//! the order the list arrived in, because a project remembers what an installer wired by that
//! number. A set that reorders its inputs after a firmware update must not move somebody's
//! cabling. The manifest's own numbering matches, so anything wired before this existed keeps
//! working.
//!
//! # Pairing is not optional
//!
//! Unlike Roku's ECP or Hisense's broker, nothing here answers a single unauthenticated call —
//! `/pairing/start` and `/pairing/pair` are the only two that do. `AUTH_TOKEN` is what every
//! other request presents in an `AUTH` header, and it is issued once, by pairing, not
//! discovered or guessed.
//!
//! # Apps: it can launch them, it cannot list them
//!
//! `PUT /app/launch` takes `{"VALUE": {NAME_SPACE, APP_ID, MESSAGE}}` and works locally like
//! every other call here. What SmartCast has no local endpoint for is the *list*: VIZIO's own
//! remote reads that from a cloud catalog, so this set can be told to open Netflix and can never
//! be asked what it has.
//!
//! Which is why nothing is hardcoded below. Core hands the payload over as `launch_id`, from the
//! catalog at <https://github.com/junohouse/apps>, and it is passed to the television **verbatim**
//! — this driver does not parse it, does not rebuild it, and has no table of its own. Three
//! reasons, and the third is the one that decided it:
//!
//! 1. The values are VIZIO's, published at `scfs.vizio.com/appservice/app_availability_prod.json`,
//!    and they change when VIZIO changes them. A correction is a pull request, not a release.
//! 2. `MESSAGE` is not decoration. Pluto TV's carries a nested Cast payload and Fandango's a URL,
//!    so anything that reduced this to a namespace and an id would break those two silently.
//! 3. A wrong id opens the wrong app — or nothing — and reports success either way. That failure
//!    is invisible from the sofa, so the ids must be ones somebody can check against VIZIO's own
//!    file rather than ones a driver author remembered.
//!
//! One known gap: Prime Video's payload differs by chipset — VIZIO's default says `NAME_SPACE 2,
//! APP_ID 4`, and five newer MediaTek panels say `NAME_SPACE 3, APP_ID 3`. The catalog carries one
//! value per app and holds the default, so Prime Video may not open on a recent set. There is no
//! local way to read the chipset, so this is recorded rather than solved.

use driver_sdk::*;
use driver_sdk::Value;

#[derive(Default)]
pub struct Vizio;

/// Fixed, because nothing about pairing needs this to vary — the TV tracks one paired identity
/// per string. A real client (the SmartCast Mobile app) does the same with its own constant.
const DEVICE_ID: &str = "juno";
const DEVICE_NAME: &str = "Juno";

const MEDIA: LocalId = 1;
const TV: LocalId = 2;

/// A key press: (codeset, code). SmartCast's remote is a fixed table of these rather than
/// named keys — see `drivers/vizio/README.md` and the module doc for where this table came
/// from (pyvizio's own, field-tested against real hardware).
type Key = (u32, u32);
const POW_ON: Key = (11, 1);
const POW_OFF: Key = (11, 0);
const POW_TOGGLE: Key = (11, 2);
const VOL_UP: Key = (5, 1);
const VOL_DOWN: Key = (5, 0);
const MUTE_ON: Key = (5, 3);
const MUTE_OFF: Key = (5, 2);
const MUTE_TOGGLE: Key = (5, 4);
const INPUT_NEXT: Key = (7, 1);
const UP: Key = (3, 8);
const DOWN: Key = (3, 0);
const LEFT: Key = (3, 1);
const RIGHT: Key = (3, 7);
const OK: Key = (3, 2);
const MENU: Key = (4, 8);
const BACK: Key = (4, 0);
const HOME: Key = (4, 15);
const PLAY: Key = (2, 3);
const PAUSE: Key = (2, 2);

const CURRENT_INPUT: &str = "/menu_native/dynamic/tv_settings/devices/current_input";

/// Every input this set has, with the flags that say which are real — see `report_inputs`.
const CURRENT_INPUTS: &str = "/menu_native/dynamic/tv_settings/devices/current_inputs";

/// The input a `set_input` is on its way to, held between asking for the hashval and spending
/// it. See the module doc: the write needs a hashval that is current *at the moment of the
/// write*, and the switch itself invalidates it — so a cached one authorises exactly one
/// switch and every one after it is refused, silently, with the TV simply not moving.
const PENDING_INPUT: &str = "pending_input";

impl Vizio {
    fn base(inst: &Instance) -> Option<String> {
        let addr = inst.property("Address").as_str()?.trim().to_string();
        if addr.is_empty() {
            return None;
        }
        Some(format!("https://{addr}:7345"))
    }

    fn auth(inst: &Instance) -> Option<String> {
        inst.property("Auth Token").as_str().filter(|s| !s.is_empty()).map(str::to_string)
    }

    fn get(inst: &Instance, path: &str) -> Option<HostCall> {
        let token = Self::auth(inst)?;
        Some(HostCall::Http(
            HttpRequest::new("GET", format!("{}{path}", Self::base(inst)?)).header("AUTH", token),
        ))
    }

    fn put(inst: &Instance, path: &str, body: Value) -> Option<HostCall> {
        let token = Self::auth(inst)?;
        Some(HostCall::Http(
            HttpRequest::new("PUT", format!("{}{path}", Self::base(inst)?))
                .json(body.to_string())
                .header("AUTH", token),
        ))
    }

    /// A stable connection id for one of the TV's inputs, derived from its `CNAME`.
    ///
    /// From the name rather than from the order the list arrived in, because a project remembers
    /// what an installer wired by this number: a set that reports its inputs in a different
    /// order after a firmware update must not move somebody's cabling. `None` for everything
    /// that is not a jack — `cast`, `watchfree`, `airplay` are apps, and the `INPUT_TYPE: 2`
    /// entries (`Player 1`, `Recorder 2`, twelve more) are CEC placeholders for devices that
    /// have never existed.
    fn connection_id(cname: &str) -> Option<LocalId> {
        if let Some(n) = cname.strip_prefix("hdmi") {
            // 1001, 1002, … — matching the manifest's own numbering, so a set that reports its
            // inputs keeps the ids anything wired before this landed was using.
            return n.parse::<LocalId>().ok().filter(|n| (1..=99).contains(n)).map(|n| 1000 + n);
        }
        match cname {
            "comp" => Some(1101),
            // The antenna jack. `hwtuner` is the same radio reported a second time on this
            // firmware, always disabled, so it is filtered out before it gets here.
            "tuner" => Some(1201),
            _ => None,
        }
    }

    /// The `CNAME` a write takes for a connection id — the inverse of [`Self::connection_id`],
    /// and the reason ids are derived from names rather than positions.
    fn cname_for(id: u64) -> Option<String> {
        match id {
            1001..=1099 => Some(format!("hdmi{}", id - 1000)),
            1101 => Some("comp".into()),
            1201 => Some("tuner".into()),
            _ => None,
        }
    }

    /// The connection id a `current_input` reading names. The same mapping again, over the
    /// display `NAME`s a reading answers with rather than the `CNAME`s a write takes — see the
    /// module doc on why the API uses each in one direction only.
    fn id_for_name(name: &str) -> Option<LocalId> {
        if let Some(n) = name.strip_prefix("HDMI-") {
            return n.parse::<LocalId>().ok().filter(|n| (1..=99).contains(n)).map(|n| 1000 + n);
        }
        match name {
            "COMP" => Some(1101),
            "Antenna" => Some(1201),
            // SMARTCAST, WatchFree+, AirPlay: the set's own apps, not jacks anything is wired to.
            _ => None,
        }
    }

    /// What kind of cable an input takes, for the pathfinder's own vocabulary.
    fn signal_class(cname: &str) -> &'static str {
        if cname.starts_with("hdmi") {
            "HDMI"
        } else if cname == "comp" {
            "COMPOSITE"
        } else {
            "RF_UHF_VHF"
        }
    }

    /// The inputs this set actually has, from a `current_inputs` reading.
    ///
    /// The manifest declares four HDMI ports because a manifest describes a product line; this
    /// V-series has three, and the fourth is a jack the pathfinder would otherwise happily route
    /// a room through. Only inputs the TV marks both `ENABLED` and `VISIBLE` count — the rest
    /// are placeholders it carries for hardware nobody owns.
    fn report_inputs(value: &Value) -> Option<HostCall> {
        let inputs = value.as_array()?;
        let connections: Vec<ConnectionDecl> = inputs
            .iter()
            .filter(|i| {
                i.get("ENABLED").and_then(Value::as_bool) == Some(true)
                    && i.get("VISIBLE").and_then(Value::as_bool) == Some(true)
            })
            .filter_map(|i| {
                let cname = i.get("CNAME").and_then(Value::as_str)?;
                Some(ConnectionDecl {
                    id: Self::connection_id(cname)?,
                    proxy: TV,
                    dir: Direction::Consumer,
                    class: Self::signal_class(cname).into(),
                    // The stock name, not `CUSTOM_NAME`: what somebody renamed an input to is
                    // theirs to change, and a connection's name is what a project was wired
                    // against.
                    name: i.get("NAME").and_then(Value::as_str)?.to_string(),
                })
            })
            .collect();
        Some(HostCall::Connections { connections })
    }

    fn send_key(inst: &Instance, (codeset, code): Key) -> Option<HostCall> {
        Self::put(
            inst,
            "/key_command/",
            json!({ "KEYLIST": [{ "CODESET": codeset, "CODE": code, "ACTION": "KEYPRESS" }] }),
        )
    }

}

impl DriverModule for Vizio {
    fn discover(&self, _driver_id: &str, state: &Value, input: &Args) -> (SetupStep, Value) {
        self.flow(state, input)
    }

    fn setup(&self, _driver_id: &str, state: &Value, input: &Args) -> (SetupStep, Value) {
        self.flow(state, input)
    }

    fn on_command(
        &self,
        inst: &mut Instance,
        proxy: LocalId,
        cmd: &str,
        args: &Args,
    ) -> Vec<HostCall> {
        if Self::base(inst).is_none() {
            return vec![HostCall::warn("vizio: set the Address on this device first")];
        }
        if Self::auth(inst).is_none() {
            return vec![HostCall::warn("vizio: this TV is not paired yet — run its setup flow")];
        }

        // --- launching an app -------------------------------------------------------------
        //
        // `launch_id` is the whole thing: VIZIO's own `app_type_payload`, handed over by core
        // from the shared catalog, and sent back to the set as the body of `PUT /app/launch`
        // unchanged. This driver never parses it — see the module header for why that matters.
        if cmd == "launch_app" {
            let Some(app) = args.get("app").and_then(Value::as_str) else {
                return vec![HostCall::warn("vizio: launch_app needs an app name")];
            };
            let Some(payload) = args.get("launch_id").and_then(Value::as_str) else {
                // The catalog has no row for it. Saying so precisely is the difference between
                // somebody adding one line to a public file and somebody filing a driver bug.
                return vec![HostCall::warn(format!(
                    "vizio: no VIZIO app id for `{app}`. This set cannot list what it has \
                     installed, so the id has to come from the catalog — add it at \
                     https://github.com/junohouse/apps"
                ))];
            };
            let Ok(value) = serde_json::from_str::<Value>(payload) else {
                return vec![HostCall::warn(format!(
                    "vizio: the catalog's entry for `{app}` is not valid JSON"
                ))];
            };

            let mut a = Args::new();
            a.insert("app".into(), Value::from(app));
            return Self::put(inst, "/app/launch", json!({ "VALUE": value }))
                .into_iter()
                .chain(std::iter::once(HostCall::notify(MEDIA, "app_changed", a)))
                .collect();
        }

        let key = match (proxy, cmd) {
            (_, "play") => PLAY,
            (_, "pause") => PAUSE,
            (_, "stop") => PAUSE, // no stop key on this remote; pause is the closest real one

            (TV, "on") => POW_ON,
            (TV, "off") => POW_OFF,
            (TV, "power_toggle") => POW_TOGGLE,
            (TV, "volume_up") => VOL_UP,
            (TV, "volume_down") => VOL_DOWN,
            (TV, "mute_toggle") => MUTE_TOGGLE,
            (TV, "set_mute") => {
                let on = args.get("mute").and_then(Value::as_bool).unwrap_or(false);
                let mut a = Args::new();
                a.insert("mute".into(), json!(on));
                let mut out = vec![];
                out.extend(Self::send_key(inst, if on { MUTE_ON } else { MUTE_OFF }));
                out.push(HostCall::notify(TV, "mute_changed", a));
                return out;
            }

            (_, "dpad") => {
                let Some(k) = args.get("key").and_then(Value::as_str) else {
                    return vec![HostCall::warn("vizio: dpad needs a key")];
                };
                match k {
                    "up" => UP,
                    "down" => DOWN,
                    "left" => LEFT,
                    "right" => RIGHT,
                    "select" => OK,
                    "back" => BACK,
                    "home" => HOME,
                    "menu" => MENU,
                    other => return vec![HostCall::warn(format!("vizio: no key `{other}`"))],
                }
            }

            (TV, "pulse_input") => {
                let mut out = vec![];
                out.extend(Self::send_key(inst, INPUT_NEXT));
                return out;
            }

            (TV, "set_input") => {
                let Some(conn) = args.get("connection").and_then(Value::as_u64) else {
                    return vec![HostCall::warn("vizio: set_input needs a connection")];
                };
                let Some(cname) = Self::cname_for(conn) else {
                    return vec![HostCall::warn(format!("vizio: no such connection {conn}"))];
                };
                // Ask, then write — see `PENDING_INPUT`. Writing from a cached hashval works
                // exactly once and then silently stops, because the switch invalidates the
                // hashval that authorised it.
                inst.scratch.insert(PENDING_INPUT.into(), json!(cname));
                return Self::get(inst, CURRENT_INPUT).into_iter().collect();
            }

            (_, other) => return vec![HostCall::warn(format!("vizio: unhandled `{other}`"))],
        };

        let mut out = Vec::new();
        out.extend(Self::send_key(inst, key));
        match cmd {
            "play" => {
                let mut a = Args::new();
                a.insert("state".into(), json!("playing"));
                out.push(HostCall::notify(MEDIA, "transport_changed", a));
            }
            "pause" | "stop" => {
                let mut a = Args::new();
                a.insert("state".into(), json!(if cmd == "pause" { "paused" } else { "stopped" }));
                out.push(HostCall::notify(MEDIA, "transport_changed", a));
            }
            "on" | "off" => {
                let mut a = Args::new();
                a.insert("on".into(), json!(cmd == "on"));
                out.push(HostCall::notify(TV, "power_changed", a));
            }
            // power_toggle, volume_up/down, mute_toggle: no optimiztic notify. Which way a
            // toggle just went, or where the level landed, is a guess this driver is not in a
            // better position to make than reading the TV back.
            _ => {}
        }
        out
    }

    fn on_event(
        &self,
        inst: &mut Instance,
        _control: LocalId,
        note: &str,
        args: &Args,
    ) -> Vec<HostCall> {
        if note != "http_response" {
            return Vec::new();
        }
        let Some(body) = args.get("body") else {
            return Vec::new();
        };
        let Some(item) = body.get("ITEMS").and_then(Value::as_array).and_then(|a| a.first())
        else {
            return Vec::new(); // a key_command reply, or nothing this driver reads
        };
        let Some(value) = item.get("VALUE") else {
            return Vec::new();
        };

        // An array: the input list. Told apart from the two scalars below by shape, like every
        // other reply here — see the module doc.
        if value.is_array() {
            return Self::report_inputs(value).into_iter().collect();
        }

        // A bare number: `power_mode`.
        if let Some(on) = value.as_i64().map(|v| v != 0) {
            let mut a = Args::new();
            a.insert("on".into(), json!(on));
            return vec![HostCall::notify(TV, "power_changed", a)];
        }

        // A bare string: `current_input`, matched on the NAME the TV reads back rather than the
        // CNAME a write takes — see the module doc.
        if let Some(name) = value.as_str() {
            let hashval = item.get("HASHVAL").and_then(Value::as_i64);
            if let Some(hashval) = hashval {
                inst.scratch.insert("current_input_hashval".into(), json!(hashval));
            }

            // A switch was waiting on exactly this. Spend the hashval immediately — anything
            // that happens in between is what makes it stale.
            if let Some(cname) = inst.scratch.remove(PENDING_INPUT) {
                let (Some(cname), Some(hashval)) = (cname.as_str(), hashval) else {
                    return vec![HostCall::warn(
                        "vizio: the TV did not say which input it is on, so there is nothing to \
                         switch from",
                    )];
                };
                let mut out = Vec::new();
                out.extend(Self::put(
                    inst,
                    CURRENT_INPUT,
                    // The CNAME, not the NAME — see the module doc.
                    json!({ "REQUEST": "MODIFY", "VALUE": cname, "HASHVAL": hashval }),
                ));
                if let Some(id) = Self::connection_id(cname) {
                    let mut a = Args::new();
                    a.insert("connection".into(), json!(id));
                    out.push(HostCall::notify(TV, "input_changed", a));
                }
                return out;
            }

            if let Some(id) = Self::id_for_name(name) {
                let mut a = Args::new();
                a.insert("connection".into(), json!(id));
                return vec![HostCall::notify(TV, "input_changed", a)];
            }
            return Vec::new();
        }

        Vec::new()
    }

    fn on_bind(&self, inst: &mut Instance) -> Vec<HostCall> {
        let mut out = Vec::new();
        let mut a = Args::new();
        a.insert("online".into(), json!(true));
        out.push(HostCall::notify(MEDIA, "online_changed", a));
        out.extend(Self::get(inst, "/state/device/power_mode"));
        out.extend(Self::get(inst, CURRENT_INPUT));
        // Which inputs this particular set has, replacing the manifest's product-line guess.
        // Asked on every bind rather than once: somebody can enable or hide an input in the
        // TV's own settings, and the manifest cannot know that either.
        out.extend(Self::get(inst, CURRENT_INPUTS));
        out
    }
}

// ---------------------------------------------------------------------------------------
// Setup flow — pairing
// ---------------------------------------------------------------------------------------

impl Vizio {
    fn flow(&self, state: &Value, input: &Args) -> (SetupStep, Value) {
        let phase = state.get("phase").and_then(Value::as_str).unwrap_or("start");
        match phase {
            "start" => {
                // Core hands over whatever the survey already found, so a set that was added
                // from Discovery arrives with its address known — see `SurveyCache::seed_for`.
                // The field stays, because multicast is blocked on plenty of networks and
                // typing it in is still the fallback; it just starts filled in.
                let found = state
                    .get("mdns_candidates")
                    .and_then(Value::as_array)
                    .and_then(|all| all.first())
                    .and_then(|c| c.get("address"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                (
                    SetupStep::Form {
                        title: "Add a VIZIO TV".into(),
                        body: if found.is_some() {
                            "The next screen asks for a code the TV shows once this reaches it, \
                             so leave the set on and pointed at that screen."
                                .into()
                        } else {
                            "Its address is on the TV: Settings → Network → Network Connection. \
                             The next screen asks for a code the TV shows once this reaches it, \
                             so leave the set on and pointed at that screen."
                                .to_string()
                        },
                        fields: vec![Field {
                            name: "address".into(),
                            label: "Address".into(),
                            kind: "string".into(),
                            help: "for example 192.168.1.42".into(),
                            default: found.map(Value::String),
                            options: Vec::new(),
                            required: true,
                        }],
                    },
                    json!({ "phase": "entered" }),
                )
            }

            "entered" => {
                let address =
                    input.get("address").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if address.is_empty() {
                    return (
                        SetupStep::Failed { reason: "an address is needed".into() },
                        Value::Null,
                    );
                }
                (
                    SetupStep::Fetch {
                        request: HttpRequest::new(
                            "PUT",
                            format!("https://{address}:7345/pairing/start"),
                        )
                        .json(json!({ "DEVICE_ID": DEVICE_ID, "DEVICE_NAME": DEVICE_NAME }).to_string()),
                        note: "asking the TV to show a pairing code".into(),
                    },
                    json!({ "phase": "started", "address": address }),
                )
            }

            "started" => {
                let address = state.get("address").and_then(Value::as_str).unwrap_or_default().to_string();
                let response = input.get("response").cloned().unwrap_or(Value::Null);
                let item = response.get("ITEM");
                let (Some(token), Some(challenge)) = (
                    item.and_then(|i| i.get("PAIRING_REQ_TOKEN")).and_then(Value::as_i64),
                    item.and_then(|i| i.get("CHALLENGE_TYPE")).and_then(Value::as_i64),
                ) else {
                    return (
                        SetupStep::Failed {
                            reason: format!(
                                "{address} did not answer as a VIZIO SmartCast TV. Check the \
                                 address under Settings → Network → Network Connection."
                            ),
                        },
                        Value::Null,
                    );
                };
                (
                    SetupStep::Form {
                        title: "Enter the code shown on the TV".into(),
                        body: "A 4-digit code should now be on screen.".into(),
                        fields: vec![Field {
                            name: "pin".into(),
                            label: "Code".into(),
                            kind: "string".into(),
                            help: String::new(),
                            default: None,
                            options: Vec::new(),
                            required: true,
                        }],
                    },
                    json!({
                        "phase": "coded", "address": address,
                        "req_token": token, "challenge_type": challenge,
                    }),
                )
            }

            "coded" => {
                let address = state.get("address").and_then(Value::as_str).unwrap_or_default().to_string();
                let req_token = state.get("req_token").and_then(Value::as_i64).unwrap_or(0);
                let challenge = state.get("challenge_type").and_then(Value::as_i64).unwrap_or(0);
                let pin = input.get("pin").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if pin.is_empty() {
                    return (SetupStep::Failed { reason: "a code is needed".into() }, Value::Null);
                }
                (
                    SetupStep::Fetch {
                        request: HttpRequest::new(
                            "PUT",
                            format!("https://{address}:7345/pairing/pair"),
                        )
                        .json(
                            json!({
                                "DEVICE_ID": DEVICE_ID,
                                "CHALLENGE_TYPE": challenge,
                                "PAIRING_REQ_TOKEN": req_token,
                                "RESPONSE_VALUE": pin,
                            })
                            .to_string(),
                        ),
                        note: "checking the code".into(),
                    },
                    json!({ "phase": "paired", "address": address }),
                )
            }

            "paired" => {
                let address = state.get("address").and_then(Value::as_str).unwrap_or_default().to_string();
                let response = input.get("response").cloned().unwrap_or(Value::Null);
                let Some(token) = response.get("ITEM").and_then(|i| i.get("AUTH_TOKEN")).and_then(Value::as_str)
                else {
                    return (
                        SetupStep::Failed {
                            reason: "that code was not accepted. Start over and check the TV \
                                     screen for a fresh one."
                                .into(),
                        },
                        Value::Null,
                    );
                };
                (
                    SetupStep::Choose {
                        title: "Add this TV".into(),
                        body: "Paired.".into(),
                        options: vec![Candidate {
                            label: format!("VIZIO TV ({address})"),
                            kind: "VIZIO TV".into(),
                            driver_id: "vizio.tv".into(),
                            properties: [
                                ("Address".to_string(), json!(address)),
                                ("Auth Token".to_string(), json!(token)),
                            ]
                            .into_iter()
                            .collect(),
                            verified: "paired and holding an auth token".into(),
                            ..Default::default()
                        }],
                        multiple: false,
                    },
                    json!({ "phase": "chosen" }),
                )
            }

            "chosen" => {
                let devices: Vec<Candidate> = input
                    .get("chosen")
                    .and_then(|c| driver_sdk::serde_json::from_value(c.clone()).ok())
                    .unwrap_or_default();
                (SetupStep::done(devices), Value::Null)
            }

            other => (
                SetupStep::Failed { reason: format!("unknown setup phase `{other}`") },
                Value::Null,
            ),
        }
    }
}


#[cfg(test)]
mod seeded_tests {
    use super::*;

    fn start(state: Value) -> SetupStep {
        Vizio.setup("vizio.tv", &state, &Args::new()).0
    }

    /// Added from Discovery, which already found the set. Asking for an address that is on
    /// screen two panels away is the wizard not being told what the pane knew.
    #[test]
    fn a_set_found_on_the_network_starts_with_its_address() {
        let step = start(json!({
            "mdns_candidates": [{ "address": "192.168.1.175", "service": "_viziocast._tcp" }]
        }));
        let SetupStep::Form { fields, .. } = step else { panic!("expected a form, got {step:?}") };
        assert_eq!(
            fields[0].default.as_ref().and_then(Value::as_str),
            Some("192.168.1.175"),
        );
    }

    /// Multicast is blocked on plenty of networks, and typing it in is still the way through.
    #[test]
    fn a_set_nobody_found_still_asks() {
        let step = start(json!({}));
        let SetupStep::Form { fields, .. } = step else { panic!("expected a form") };
        assert!(fields[0].default.is_none());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_refused_before_pairing_rather_than_sent_unauthenticated() {
        let driver = Vizio;
        let mut inst = Instance::default();
        inst.properties.insert("Address".into(), json!("10.0.0.5"));
        let calls = driver.on_command(&mut inst, TV, "on", &Args::new());
        assert!(
            matches!(calls.as_slice(), [HostCall::Log { level, .. }] if level == "warn"),
            "expected a warning, got {calls:?}"
        );
    }

    #[test]
    fn power_on_sends_the_documented_key_code() {
        let driver = Vizio;
        let mut inst = Instance::default();
        inst.properties.insert("Address".into(), json!("10.0.0.5"));
        inst.properties.insert("Auth Token".into(), json!("tok"));
        let calls = driver.on_command(&mut inst, TV, "on", &Args::new());
        let [HostCall::Http(req), HostCall::Notify { .. }] = calls.as_slice() else {
            panic!("expected an http call and a notify, got {calls:?}");
        };
        assert!(req.url.ends_with("/key_command/"));
        assert!(req.headers.iter().any(|(k, v)| k == "AUTH" && v == "tok"));
        let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["KEYLIST"][0]["CODESET"], 11);
        assert_eq!(body["KEYLIST"][0]["CODE"], 1);
    }

    /// The payload goes to the television exactly as the catalog holds it.
    ///
    /// Not rebuilt from parts, and that is the test. `MESSAGE` carries a nested Cast blob for
    /// Pluto and a URL for Fandango, so anything that took this apart into a namespace and an id
    /// would drop those and open a home screen while reporting success.
    #[test]
    fn a_catalog_payload_is_sent_to_the_set_untouched() {
        let driver = Vizio;
        let mut inst = paired();
        let mut args = Args::new();
        args.insert("app".into(), json!("Netflix"));
        args.insert(
            "launch_id".into(),
            json!(r#"{"NAME_SPACE":3,"APP_ID":"1","MESSAGE":null}"#),
        );

        let calls = driver.on_command(&mut inst, MEDIA, "launch_app", &args);
        let [HostCall::Http(req), HostCall::Notify { .. }] = calls.as_slice() else {
            panic!("expected an http call and a notify, got {calls:?}");
        };
        assert!(req.url.ends_with("/app/launch"));
        assert!(req.headers.iter().any(|(k, v)| k == "AUTH" && v == "tok"));
        let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["VALUE"]["NAME_SPACE"], 3);
        assert_eq!(body["VALUE"]["APP_ID"], "1");
        assert!(body["VALUE"]["MESSAGE"].is_null());
    }

    /// A `MESSAGE` that is itself a document survives, because it is never looked at.
    #[test]
    fn a_message_payload_survives_whole() {
        let driver = Vizio;
        let mut inst = paired();
        let mut args = Args::new();
        args.insert("app".into(), json!("Pluto TV"));
        args.insert(
            "launch_id".into(),
            json!(r#"{"NAME_SPACE":0,"APP_ID":"E6F74C01","MESSAGE":{"CAST_NAMESPACE":"urn:x-cast:tv.pluto"}}"#),
        );

        let calls = driver.on_command(&mut inst, MEDIA, "launch_app", &args);
        let [HostCall::Http(req), ..] = calls.as_slice() else {
            panic!("expected an http call, got {calls:?}");
        };
        let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["VALUE"]["MESSAGE"]["CAST_NAMESPACE"], "urn:x-cast:tv.pluto");
    }

    /// No id means say so, not send something. This set cannot be asked what it has, so an app
    /// the catalog has never heard of has nowhere else to come from — and a silent no-op here
    /// would look exactly like a television that ignored the remote.
    #[test]
    fn an_app_the_catalog_does_not_know_is_refused_out_loud() {
        let driver = Vizio;
        let mut inst = paired();
        let mut args = Args::new();
        args.insert("app".into(), json!("Some Regional Broadcaster"));

        let calls = driver.on_command(&mut inst, MEDIA, "launch_app", &args);
        let [HostCall::Log { level, msg }] = calls.as_slice() else {
            panic!("expected one warning, got {calls:?}");
        };
        assert_eq!(level, "warn");
        assert!(msg.contains("junohouse/apps"), "say where to fix it: {msg}");
    }

    fn current_input(name: &str, hashval: i64) -> Args {
        let mut a = Args::new();
        a.insert(
            "body".into(),
            json!({ "ITEMS": [{ "CNAME": "current_input", "VALUE": name, "HASHVAL": hashval }] }),
        );
        a
    }

    fn paired() -> Instance {
        let mut inst = Instance::default();
        inst.properties.insert("Address".into(), json!("10.0.0.5"));
        inst.properties.insert("Auth Token".into(), json!("tok"));
        inst
    }

    /// The whole of `set_input`, both halves. It asks first and writes from the answer, and the
    /// write takes the lowercase `CNAME` — a real set refuses the display `NAME` with
    /// `FAILURE`. Both are the kind of thing that reads like a typo and would be "corrected"
    /// straight back into a bug; both were found against real hardware.
    #[test]
    fn set_input_asks_for_a_fresh_hashval_then_writes_the_cname() {
        let driver = Vizio;
        let mut inst = paired();

        let mut a = Args::new();
        a.insert("connection".into(), json!(1002u64));
        let calls = driver.on_command(&mut inst, TV, "set_input", &a);
        let [HostCall::Http(ask)] = calls.as_slice() else {
            panic!("expected one read and no write yet, got {calls:?}");
        };
        assert_eq!(ask.method, "GET");
        assert!(ask.url.ends_with("/current_input"));

        // The answer carries the hashval that authorises the write.
        let calls = driver.on_event(&mut inst, 0, "http_response", &current_input("SMARTCAST", 42));
        let [HostCall::Http(write), HostCall::Notify { name, args, .. }] = calls.as_slice() else {
            panic!("expected the write and a notify, got {calls:?}");
        };
        assert_eq!(write.method, "PUT");
        let body: Value = serde_json::from_str(write.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["VALUE"], "hdmi2", "a write takes the CNAME");
        assert_eq!(body["HASHVAL"], 42, "and the hashval that just came back");
        assert_eq!(name, "input_changed");
        assert_eq!(args.get("connection").unwrap(), &json!(1002));
        assert!(!inst.scratch.contains_key(PENDING_INPUT), "the switch is done");
    }

    /// Switching twice in a row. The hashval is invalidated by the switch it authorises, so a
    /// driver that reuses one has its second write refused and the TV simply does not move —
    /// silently, since nothing here reads `STATUS`. Caught live: the first switch worked and
    /// every one after it did nothing.
    #[test]
    fn a_second_switch_asks_again_rather_than_reusing_a_spent_hashval() {
        let driver = Vizio;
        let mut inst = paired();

        for (conn, hashval, cname) in [(1002u64, 42i64, "hdmi2"), (1003, 77, "hdmi3")] {
            let mut a = Args::new();
            a.insert("connection".into(), json!(conn));
            let calls = driver.on_command(&mut inst, TV, "set_input", &a);
            assert!(
                matches!(calls.as_slice(), [HostCall::Http(r)] if r.method == "GET"),
                "every switch starts by asking, got {calls:?}"
            );
            let calls =
                driver.on_event(&mut inst, 0, "http_response", &current_input("SMARTCAST", hashval));
            let [HostCall::Http(write), ..] = calls.as_slice() else {
                panic!("expected a write, got {calls:?}");
            };
            let body: Value = serde_json::from_str(write.body.as_deref().unwrap()).unwrap();
            assert_eq!(body["HASHVAL"], hashval, "each write spends the hashval just read");
            assert_eq!(body["VALUE"], cname);
        }
    }

    /// The other direction: a reading answers the display `NAME`, so that is what an unprompted
    /// reply is matched on. Matching a reading against the CNAME would report nothing, ever.
    #[test]
    fn a_reading_nobody_asked_for_is_matched_on_the_display_name() {
        let driver = Vizio;
        let mut inst = Instance::default();
        let calls = driver.on_event(&mut inst, 0, "http_response", &current_input("HDMI-2", 99));
        let [HostCall::Notify { name, args, .. }] = calls.as_slice() else {
            panic!("expected input_changed, got {calls:?}");
        };
        assert_eq!(name, "input_changed");
        assert_eq!(args.get("connection").unwrap(), &json!(1002));
    }

    /// An input somebody renamed in the TV's own settings still answers its stock `NAME` on a
    /// reading — `CUSTOM_NAME` is cosmetic and matched on nothing.
    #[test]
    fn a_renamed_input_still_matches_on_its_stock_name() {
        let driver = Vizio;
        let mut inst = Instance::default();
        let calls = driver.on_event(&mut inst, 0, "http_response", &current_input("Videocore", 1));
        assert!(calls.is_empty(), "a custom name matches nothing, got {calls:?}");
    }

    #[test]
    fn a_power_reading_becomes_a_power_changed_notification() {
        let driver = Vizio;
        let mut inst = Instance::default();
        let mut a = Args::new();
        a.insert("body".into(), json!({ "ITEMS": [{ "CNAME": "power_mode", "VALUE": 1 }] }));
        let calls = driver.on_event(&mut inst, 0, "http_response", &a);
        let [HostCall::Notify { name, args, .. }] = calls.as_slice() else {
            panic!("expected one power_changed notification, got {calls:?}");
        };
        assert_eq!(name, "power_changed");
        assert_eq!(args.get("on").unwrap(), &json!(true));
    }

    #[test]
    fn a_key_command_reply_carries_no_items_and_is_quietly_ignored() {
        let driver = Vizio;
        let mut inst = Instance::default();
        let mut a = Args::new();
        a.insert("body".into(), json!({ "STATUS": { "RESULT": "SUCCESS" } }));
        assert!(driver.on_event(&mut inst, 0, "http_response", &a).is_empty());
    }
    /// The real `current_inputs` payload from a V505-H19, trimmed to the entries that matter.
    /// Its manifest declares four HDMI ports; this set has three, and the fourth is a jack the
    /// pathfinder would otherwise route a room through.
    #[test]
    fn only_real_jacks_are_reported_as_connections() {
        let driver = Vizio;
        let mut inst = Instance::default();
        let mut a = Args::new();
        a.insert(
            "body".into(),
            json!({ "ITEMS": [{ "CNAME": "current_inputs", "VALUE": [
                { "CNAME": "hwtuner",  "NAME": "tuner",      "ENABLED": false, "VISIBLE": false },
                { "CNAME": "comp",     "NAME": "COMP",       "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "hdmi1",    "NAME": "HDMI-1",     "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "hdmi2",    "NAME": "HDMI-2",     "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "hdmi3",    "NAME": "HDMI-3",     "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "usb",      "NAME": "usb",        "ENABLED": false, "VISIBLE": false },
                { "CNAME": "cast",     "NAME": "SMARTCAST",  "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "watchfree","NAME": "WatchFree+", "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "Player 1", "NAME": "Player 1",   "ENABLED": false, "VISIBLE": false },
                { "CNAME": "airplay",  "NAME": "AirPlay",    "ENABLED": true,  "VISIBLE": true  },
                { "CNAME": "tuner",    "NAME": "Antenna",    "ENABLED": true,  "VISIBLE": true  }
            ] }] }),
        );
        let calls = driver.on_event(&mut inst, 0, "http_response", &a);
        let [HostCall::Connections { connections }] = calls.as_slice() else {
            panic!("expected one Connections call, got {calls:?}");
        };

        let ids: Vec<LocalId> = connections.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1101, 1001, 1002, 1003, 1201], "three HDMI, composite, antenna");
        assert!(
            !ids.contains(&1004),
            "the manifest's fourth HDMI is exactly the phantom this exists to remove"
        );

        // Apps and CEC placeholders are not jacks, and a disabled duplicate of the tuner is not
        // a second antenna socket.
        assert_eq!(connections.len(), 5);
        assert!(connections.iter().all(|c| c.dir == Direction::Consumer && c.proxy == TV));
        let hdmi2 = connections.iter().find(|c| c.id == 1002).unwrap();
        assert_eq!(hdmi2.class, "HDMI");
        assert_eq!(hdmi2.name, "HDMI-2");
        assert_eq!(connections.iter().find(|c| c.id == 1201).unwrap().class, "RF_UHF_VHF");
    }

    /// Ids come from the name, so a set that reports its inputs in a different order after a
    /// firmware update does not renumber somebody's cabling.
    #[test]
    fn connection_ids_do_not_depend_on_the_order_inputs_arrive_in() {
        assert_eq!(Vizio::connection_id("hdmi2"), Some(1002));
        assert_eq!(Vizio::cname_for(1002).as_deref(), Some("hdmi2"));
        assert_eq!(Vizio::id_for_name("HDMI-2"), Some(1002));
        // Apps have no id in either direction.
        assert_eq!(Vizio::connection_id("cast"), None);
        assert_eq!(Vizio::id_for_name("SMARTCAST"), None);
    }
}

export_driver!(Vizio);
