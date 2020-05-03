pub(crate) async fn zulip_task() {
    let client = reqwest::Client::new();
    let queue_register_res = client
        .post("https://rust-lang.zulipchat.com/api/v1/register")
        .basic_auth(crate::ZULIP_USER, Some(crate::ZULIP_TOKEN))
        .body("event_types=%5B%22message%22%5D")
        .send().await.unwrap()
        .text().await.unwrap();
    let queue_id = (serde_json::from_str(&queue_register_res) as Result<serde_json::Value, _>).unwrap()
        .as_object().unwrap()["queue_id"].as_str().unwrap().to_string();
    //let queue_id = "1588463074:5047";
    println!("zulip queue: {}", queue_id);
    let mut last_event_id = -1;
    loop {
        let url = format!("https://rust-lang.zulipchat.com/api/v1/events?queue_id={}&last_event_id={}&dont_block=false", queue_id.replace(':', "%3A"), last_event_id);
        println!("GET {}", url);
        let events = client.get(&url)
            .basic_auth(crate::ZULIP_USER, Some(crate::ZULIP_TOKEN))
            .send().await.unwrap()
            .text().await.unwrap();
        let events: ZulipEvents = serde_json::from_str(&events).unwrap_or_else(|e| {
            panic!("{:?}: {}", e, events)
        });
        for event in events.events {
            match event {
                ZulipEvent::Message { id, message } => {
                    println!("{:?}", message);
                    let _ = crate::parse_comment("zulip", message.sender_id, message.id, &message.content).await;
                    last_event_id = id as i64;
                }
                _ => {}
            }
        }
        tokio::time::delay_for(std::time::Duration::from_secs(1)).await;
    }
}

pub(crate) async fn zulip_post_message(user: u64, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!("https://rust-lang.zulipchat.com/api/v1/messages?type=private&to=%5B{}%5D&content={}", user, percent_encoding::utf8_percent_encode(body, percent_encoding::NON_ALPHANUMERIC)))
        .basic_auth(crate::ZULIP_USER, Some(crate::ZULIP_TOKEN))
        .send().await?
        .text().await?;
    println!("post message result: {}", res);
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct ZulipEvents {
    result: String,
    events: Vec<ZulipEvent>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type")]
enum ZulipEvent {
    #[serde(rename = "message")]
    Message {
        id: u64,
        message: ZulipMessage,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, serde::Deserialize)]
struct ZulipMessage {
    id: u64,
    content: String,
    sender_full_name: String,
    sender_id: u64,
}
