use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, time::{SystemTime, UNIX_EPOCH}};
use reqwest::{Client, multipart};
use tokio::time::{sleep, Duration};

#[derive(Deserialize, Debug)]
struct CatalogPage {
    threads: Vec<Thread>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct Thread {
    #[serde(rename = "no")]
    id: u64,
    replies: u32,
    #[serde(default)]
    time: u64,
    sub: Option<String>,
    com: Option<String>,
    tim: Option<u64>,
    ext: Option<String>,
}

fn shared_clean(input: &str) -> String {
    let re_spans = regex::Regex::new(r"<span[^>]*>|</span>").unwrap();
    let no_spans = re_spans.replace_all(input, "");
    let no_embed = no_spans.replace(" [Embed]", "").replace("[Embed]", "");
    no_embed
        .replace("<br>", "\n")
        .replace("<wbr>", "")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#039;", "'")
}

fn clean_text_discord(input: &str) -> String {
    let base = shared_clean(input);
    let re_html_links = regex::Regex::new(r"<a [^>]*>(.*?)</a>").unwrap();
    let no_html_links = re_html_links.replace_all(&base, "$1");
    let re_links = regex::Regex::new(r"https?://[^\s<>]+").unwrap();
    re_links.replace_all(&no_html_links, |caps: &regex::Captures| {
        format!("`{}`", &caps[0])
    }).to_string()
}

fn clean_text_telegram(input: &str) -> String {
    shared_clean(input)
}

fn generate_smart_title(thread: &Thread) -> String {
    let mut title = if let Some(ref s) = thread.sub {
        if !s.is_empty() && s != "POL THREAD" {
            s.to_string()
        } else {
            self_generate_title(thread)
        }
    } else {
        self_generate_title(thread)
    };
    title = title
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#039;", "'");
    title
}

fn self_generate_title(thread: &Thread) -> String {
    if let Some(ref c) = thread.com {
        let cleaned = clean_text_telegram(c);
        let first_line = cleaned.lines().next().unwrap_or("").trim();
        if !first_line.is_empty() {
            let mut truncated = first_line.chars().take(200).collect::<String>();
            if first_line.chars().count() > 200 {
                truncated.push_str("...");
            }
            return truncated;
        }
    }
    format!("Thread #{}", thread.id)
}

async fn post_to_telegram(client: &Client, token: &str, chat_id: &str, thread: &Thread, file_bytes: Option<Vec<u8>>) -> Result<(), Box<dyn std::error::Error>> {
    let smart_title = generate_smart_title(thread);
    let mut comment = thread.com.as_deref().map(clean_text_telegram).unwrap_or_else(|| "".to_string());
    let thread_url = format!("https://boards.4channel.org/pol/thread/{}", thread.id);
    let archive_url = format!("https://archive.4plebs.org/pol/thread/{}", thread.id);
    let footer = format!("\n\n💬 <b>Replies: {}</b>\n🔗 <a href='{}'>Thread Link</a>\n🏛️ <a href='{}'>4plebs Archive</a>", thread.replies, thread_url, archive_url);
    let header = format!("<b>{}</b>\n\n", smart_title);
    
    let max_comment_len = 1024_i32 - (header.len() as i32) - (footer.len() as i32) - 10;
    let max_comment_len = if max_comment_len < 0 { 0 } else { max_comment_len as usize };
    if comment.len() > max_comment_len {
        comment = comment.chars().take(max_comment_len).collect::<String>();
        comment.push_str("...");
    }
    let caption = format!("{}{}{}", header, comment, footer);

    if let (Some(bytes), Some(ext)) = (file_bytes, &thread.ext) {
        if !bytes.is_empty() {
            // Renaming webm to mp4 tricks the Telegram client into opening the video player
            let (method, field, upload_name) = match ext.as_str() {
                ".webm" | ".mp4" => ("sendVideo", "video", "video.mp4"),
                ".gif" => ("sendAnimation", "animation", "animation.gif"),
                _ => ("sendPhoto", "photo", "image.jpg"),
            };
            
            let url = format!("https://api.telegram.org/bot{}/{}", token, method);
            let part = multipart::Part::bytes(bytes).file_name(upload_name);
            let mut form = multipart::Form::new()
                .text("chat_id", chat_id.to_string())
                .text("caption", caption.clone())
                .text("parse_mode", "HTML")
                .part(field, part);

            if method == "sendVideo" {
                form = form.text("supports_streaming", "true");
            }

            let res = client.post(url).multipart(form).send().await?;
            if res.status().is_success() { return Ok(()); }
        }
    }

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    client.post(url).json(&serde_json::json!({ "chat_id": chat_id, "text": caption, "parse_mode": "HTML", "disable_web_page_preview": false })).send().await?;
    Ok(())
}

async fn post_to_discord(client: &Client, webhook: &str, thread: &Thread, file_bytes: Option<Vec<u8>>) -> Result<(), Box<dyn std::error::Error>> {
    let smart_title = generate_smart_title(thread);
    let mut comment = thread.com.as_deref().map(clean_text_discord).unwrap_or_else(|| "".to_string());
    let thread_url = format!("https://boards.4channel.org/pol/thread/{}", thread.id);
    let archive_url = format!("https://archive.4plebs.org/pol/thread/{}", thread.id);
    let header = format!("## {}\n", smart_title);
    let footer = format!("\n\n**Replies:** {}\n**Thread Link:** <{}>\n**Archive:** <{}>", thread.replies, thread_url, archive_url);
    let max_comment_len = 2000_i32 - (header.len() as i32) - (footer.len() as i32) - 10;
    let max_comment_len = if max_comment_len < 0 { 0 } else { max_comment_len as usize };
    if comment.len() > max_comment_len {
        comment = comment.chars().take(max_comment_len).collect::<String>();
        comment.push_str("...");
    }
    let content = format!("{}{}{}", header, comment, footer);
    let mut form = multipart::Form::new().text("payload_json", serde_json::json!({ "content": content }).to_string());
    if let (Some(bytes), Some(ext)) = (file_bytes, &thread.ext) {
        if !bytes.is_empty() {
            let part = multipart::Part::bytes(bytes).file_name(format!("file{}", ext));
            form = form.part("file", part);
        }
    }
    client.post(webhook).multipart(form).send().await?;
    Ok(())
}

fn get_posted_ids() -> HashSet<u64> {
    let data = fs::read_to_string("posted.json").unwrap_or_else(|_| "[]".to_string());
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_posted_id(id: u64) {
    let mut ids = get_posted_ids();
    ids.insert(id);
    let _ = fs::write("posted.json", serde_json::to_string(&ids).unwrap());
}

async fn process_thread(client: &Client, discord_url: &str, tg_token: &str, tg_chat: &str, thread: &Thread) -> Result<(), Box<dyn std::error::Error>> {
    let mut file_bytes = None;
    if let (Some(tim), Some(ext)) = (thread.tim, &thread.ext) {
        let file_url = format!("https://i.4cdn.org/pol/{}{}", tim, ext);
        if let Ok(resp) = client.get(file_url).header("Referer", "https://boards.4channel.org/").send().await {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes().await { file_bytes = Some(bytes.to_vec()); }
            }
        }
    }
    let _ = post_to_discord(client, discord_url, thread, file_bytes.clone()).await;
    let _ = post_to_telegram(client, tg_token, tg_chat, thread, file_bytes).await;
    save_posted_id(thread.id);
    Ok(())
}

async fn run_native_archive_cold_start(client: &Client, discord_url: &str, tg_token: &str, tg_chat: &str) -> Result<(), Box<dyn std::error::Error>> {
    let archive_ids: Vec<u64> = client.get("https://a.4cdn.org/pol/archive.json").send().await?.json().await?;
    let posted_ids = get_posted_ids();
    for &id in archive_ids.iter().take(100) {
        if posted_ids.contains(&id) { continue; }
        let thread_url = format!("https://a.4cdn.org/pol/thread/{}.json", id);
        if let Ok(resp) = client.get(thread_url).send().await {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                if let Some(op) = data["posts"].get(0) {
                    let replies = op["replies"].as_u64().unwrap_or(0);
                    if replies >= 300 {
                        let thread = Thread { id, replies: replies as u32, time: op["time"].as_u64().unwrap_or(0), sub: op["sub"].as_str().map(|s| s.to_string()), com: op["com"].as_str().map(|s| s.to_string()), tim: op["tim"].as_u64(), ext: op["ext"].as_str().map(|s| s.to_string()) };
                        let _ = process_thread(client, discord_url, tg_token, tg_chat, &thread).await;
                    }
                }
            }
        }
        sleep(Duration::from_millis(500)).await;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let discord_webhook = std::env::var("POL_WEBHOOK")?;
    let tg_token = std::env::var("TELEGRAM_TOKEN")?;
    let tg_chat = std::env::var("TELEGRAM_CHAT_ID")?;
    let client = Client::builder().user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36").build()?;
    let _ = run_native_archive_cold_start(&client, &discord_webhook, &tg_token, &tg_chat).await;
    loop {
        let posted_ids = get_posted_ids();
        if let Ok(response) = client.get("https://a.4cdn.org/pol/catalog.json").send().await {
            if let Ok(pages) = response.json::<Vec<CatalogPage>>().await {
                for page in pages {
                    for thread in page.threads {
                        if thread.replies >= 300 && !posted_ids.contains(&thread.id) {
                            let _ = process_thread(&client, &discord_webhook, &tg_token, &tg_chat, &thread).await;
                            sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        }
        sleep(Duration::from_secs(300)).await;
    }
}
