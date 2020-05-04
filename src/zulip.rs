async fn register_event_queue(client: &reqwest::Client) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let res = client
        .post("https://rust-lang.zulipchat.com/api/v1/register")
        .basic_auth(crate::ZULIP_USER, Some(crate::ZULIP_TOKEN))
        .body("event_types=%5B%22message%22%5D")
        .send().await?
        .text().await?;
    let res: serde_json::Value = serde_json::from_str(&res)?;
    let queue_id = res.as_object().unwrap()["queue_id"].as_str().unwrap().to_string();
    println!("zulip queue: {}", queue_id);
    Ok(queue_id)
}

pub(crate) async fn zulip_task() {
    let client = reqwest::Client::new();
    let mut queue_id = register_event_queue(&client).await.unwrap();
    let mut last_event_id = -1;
    loop {
        let url = format!("https://rust-lang.zulipchat.com/api/v1/events?queue_id={}&last_event_id={}&dont_block=false", queue_id.replace(':', "%3A"), last_event_id);
        println!("GET {}", url);
        let events_json = client.get(&url)
            .basic_auth(crate::ZULIP_USER, Some(crate::ZULIP_TOKEN))
            .send().await.unwrap()
            .text().await.unwrap();
        if events_json.contains("BAD_EVENT_QUEUE_ID") {
            // Event queue is garbage collected
            queue_id = register_event_queue(&client).await.unwrap();
            continue;
        }
        let events: ZulipEvents = serde_json::from_str(&events_json).unwrap_or_else(|e| {
            panic!("{:?}: {}", e, events_json)
        });
        for event in events.events {
            match event {
                ZulipEvent::Heartbeat { id } => last_event_id = id as i64,
                ZulipEvent::Message { id, message } => {
                    println!("{:?}", message);
                    if let Some(stream_id) = message.stream_id {
                        let _ = crate::parse_comment(
                            &crate::ReplyTo::ZulipPublic { stream_id, subject: message.subject },
                            &format!("zulip{}", message.id),
                            &message.content,
                        ).await;
                    } else {
                        let _ = crate::parse_comment(
                            &crate::ReplyTo::ZulipPrivate { user_id: message.sender_id },
                            &format!("zulip{}", message.id),
                            &message.content,
                        ).await;
                    }
                    last_event_id = id as i64;
                }
                ZulipEvent::Pointer { id } => last_event_id = id as i64,
                ZulipEvent::Presence { id } => last_event_id = id as i64,
                ZulipEvent::Typing { id } => last_event_id = id as i64,
                ZulipEvent::UpdateMessageFlags { id } => last_event_id = id as i64,
                ZulipEvent::RealUser { id } => last_event_id = id as i64,
                ZulipEvent::Subscription { id } => last_event_id = id as i64,
                ZulipEvent::UpdateMessage { id } => last_event_id = id as i64,
                ZulipEvent::Reaction { id } => last_event_id = id as i64,
                ZulipEvent::Other => {
                    println!("{:?}", events_json)
                }
            }
        }
        tokio::time::delay_for(std::time::Duration::from_secs(1)).await;
    }
}

pub(crate) async fn zulip_post_public_message(stream_id: u64, subject: &str, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!(
            "https://rust-lang.zulipchat.com/api/v1/messages?type=stream&to=%5B{}%5D&subject={}&content={}",
            stream_id,
            percent_encoding::utf8_percent_encode(subject, percent_encoding::NON_ALPHANUMERIC),
            percent_encoding::utf8_percent_encode(body, percent_encoding::NON_ALPHANUMERIC),
        ))
        .basic_auth(crate::ZULIP_USER, Some(crate::ZULIP_TOKEN))
        .send().await?
        .text().await?;
    println!("post message result: {}", res);
    Ok(())
}

pub(crate) async fn zulip_post_private_message(user_id: u64, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!(
            "https://rust-lang.zulipchat.com/api/v1/messages?type=private&to=%5B{}%5D&content={}",
            user_id,
            percent_encoding::utf8_percent_encode(body, percent_encoding::NON_ALPHANUMERIC),
        ))
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
    #[serde(rename = "heartbeat")]
    Heartbeat {
        id: u64,
    },
    #[serde(rename = "message")]
    Message {
        id: u64,
        message: ZulipMessage,
    },
    #[serde(rename = "pointer")]
    Pointer {
        id: u64,
    },
    #[serde(rename = "presence")]
    Presence {
        id: u64,
    },
    #[serde(rename = "typing")]
    Typing {
        id: u64,
    },
    #[serde(rename = "update_message_flags")]
    UpdateMessageFlags {
        id: u64,
    },
    #[serde(rename = "realm_user")]
    RealUser {
        id: u64,
    },
    #[serde(rename = "subscription")]
    Subscription {
        id: u64,
    },
    #[serde(rename = "update_message")]
    UpdateMessage {
        id: u64,
    },
    #[serde(rename = "reaction")]
    Reaction {
        id: u64,
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
    #[serde(rename = "type")]
    type_: String, // private or stream
    #[serde(default)]
    stream_id: Option<u64>,
    subject: String,
}
