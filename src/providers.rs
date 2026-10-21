// Models from other companies, or running on your own computer, for the AI assistant.
//
// Most services and local runners speak the OpenAI "chat completions" format:
// OpenAI, Google Gemini, OpenRouter, Groq, Mistral, DeepSeek, and on your own
// machine Ollama, LM Studio, llama.cpp's server and vLLM. The assistant's loop
// keeps its conversation in the Anthropic Messages shape; this module converts
// a request to chat completions and the reply back, so the loop and the tools
// are the same whichever model runs them.

use serde_json::{json, Value};

// (label, base URL, needs an API key)
pub const PRESETS: [(&str, &str, bool); 9] = [
    ("OpenAI", "https://api.openai.com/v1", true),
    ("Google Gemini", "https://generativelanguage.googleapis.com/v1beta/openai", true),
    ("OpenRouter (many models)", "https://openrouter.ai/api/v1", true),
    ("Groq", "https://api.groq.com/openai/v1", true),
    ("Mistral", "https://api.mistral.ai/v1", true),
    ("DeepSeek", "https://api.deepseek.com/v1", true),
    ("Ollama (this computer)", "http://localhost:11434/v1", false),
    ("LM Studio (this computer)", "http://localhost:1234/v1", false),
    ("llama.cpp / vLLM (this computer)", "http://localhost:8080/v1", false),
];

pub fn preset_label(base_url: &str) -> &'static str {
    let url = base_url.trim().trim_end_matches('/');
    PRESETS.iter().find(|(_, u, _)| *u == url).map_or("Custom server", |p| p.0)
}

pub fn is_local(base_url: &str) -> bool {
    let rest = base_url.trim().split("://").nth(1).unwrap_or("");
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]") || host.ends_with(".local")
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn images_of(content: &Value) -> Vec<String> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "image")
        .filter_map(|b| Some(format!("data:{};base64,{}", b["source"]["media_type"].as_str()?, b["source"]["data"].as_str()?)))
        .collect()
}

// An Anthropic Messages request body as a chat completions request. Images (from
// view_scene) go along only when the model takes them (`vision`).
pub fn chat_request(body: &Value, vision: bool) -> Value {
    let mut messages = Vec::new();
    let system = text_of(&body["system"]);
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    for m in body["messages"].as_array().into_iter().flatten() {
        let content = &m["content"];
        if m["role"] == "assistant" {
            let calls: Vec<Value> = content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "tool_use")
                .map(|b| json!({"id": b["id"], "type": "function", "function": {"name": b["name"], "arguments": b["input"].to_string()}}))
                .collect();
            let text = text_of(content);
            let mut message = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
            if !calls.is_empty() {
                message["tool_calls"] = json!(calls);
            }
            messages.push(message);
            continue;
        }
        let Some(blocks) = content.as_array() else {
            messages.push(json!({"role": "user", "content": text_of(content)}));
            continue;
        };
        // Tool results become "tool" messages (they must follow the assistant's calls);
        // what's left, and any images, follows as a user message.
        let mut images = images_of(content);
        for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
            let mut text = text_of(&b["content"]);
            let pictures = images_of(&b["content"]);
            if !pictures.is_empty() {
                if vision {
                    text.push_str("\n(The image follows in the next message.)");
                } else {
                    text.push_str("\n(This model can't see images, so the picture was left out: check the result with inspect_node.)");
                }
            }
            if b["is_error"] == true {
                text = format!("Error: {text}");
            }
            messages.push(json!({"role": "tool", "tool_call_id": b["tool_use_id"], "content": text}));
            images.extend(pictures);
        }
        let text = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
        if vision && !images.is_empty() {
            let mut parts = vec![json!({"type": "text", "text": if text.is_empty() { "The image from view_scene:".to_owned() } else { text }})];
            parts.extend(images.iter().map(|url| json!({"type": "image_url", "image_url": {"url": url}})));
            messages.push(json!({"role": "user", "content": parts}));
        } else if !text.is_empty() {
            messages.push(json!({"role": "user", "content": text}));
        }
    }
    let tools: Vec<Value> = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| vision || t["name"] != "view_scene")
        .map(|t| json!({"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["input_schema"]}}))
        .collect();
    json!({
        "model": body["model"],
        "max_tokens": body["max_tokens"],
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
    })
}

// A chat completions reply in the Anthropic Messages shape the loop reads:
// content blocks (text, tool_use), stop_reason and usage.
pub fn from_chat(reply: &Value) -> Result<Value, String> {
    let choice = reply["choices"].get(0).ok_or_else(|| format!("the reply had no answer in it: {}", short(&reply.to_string())))?;
    let message = &choice["message"];
    let mut content = Vec::new();
    let text = text_of(&message["content"]);
    if !text.trim().is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
    for (i, call) in calls.iter().enumerate() {
        let function = &call["function"];
        // Arguments are a JSON string (some local runners send an object).
        let input = match &function["arguments"] {
            Value::String(s) if s.trim().is_empty() => json!({}),
            Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({"unreadable_arguments": s})),
            Value::Object(_) => function["arguments"].clone(),
            _ => json!({}),
        };
        let id = call["id"].as_str().filter(|s| !s.is_empty()).map_or_else(|| format!("call_{i}"), str::to_owned);
        content.push(json!({"type": "tool_use", "id": id, "name": function["name"], "input": input}));
    }
    let stop_reason = if !calls.is_empty() {
        "tool_use"
    } else {
        match choice["finish_reason"].as_str().unwrap_or("stop") {
            "length" => "max_tokens",
            "content_filter" => "refusal",
            _ => "end_turn",
        }
    };
    let usage = &reply["usage"];
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let cached = n(&usage["prompt_tokens_details"]["cached_tokens"]);
    Ok(json!({
        "content": content,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": n(&usage["prompt_tokens"]).saturating_sub(cached),
            "cache_read_input_tokens": cached,
            "output_tokens": n(&usage["completion_tokens"]),
        },
    }))
}

fn short(s: &str) -> String {
    if s.chars().count() > 300 { format!("{}…", s.chars().take(300).collect::<String>()) } else { s.to_owned() }
}

fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}/{path}", base_url.trim().trim_end_matches('/'))
}

fn unreachable(base_url: &str, e: impl std::fmt::Display) -> String {
    if is_local(base_url) {
        format!("Couldn't reach {base_url}: is the model server (e.g. Ollama or LM Studio) running? ({e})")
    } else {
        format!("Couldn't reach {base_url}: {e}")
    }
}

fn error_message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().or_else(|| v["error"].as_str()).or_else(|| v["message"].as_str()).map(str::to_owned))
        .unwrap_or_else(|| short(text))
}

// One chat completions call, with the request and reply converted. Retries dropped
// connections, rate limits and overloads; `max_tokens` is renamed for services that want
// `max_completion_tokens`.
pub fn call(agent: &ureq::Agent, base_url: &str, key: &str, body: &Value, vision: bool, cancel: &std::sync::atomic::AtomicBool) -> Result<Value, String> {
    let url = endpoint(base_url, "chat/completions");
    let mut request_body = chat_request(body, vision);
    let mut wait = 2u64;
    let mut renamed = false;
    for attempt in 0..5 {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let mut request = agent.post(&url).header("content-type", "application/json");
        if !key.trim().is_empty() {
            request = request.header("authorization", &format!("Bearer {}", key.trim()));
        }
        match request.send_json(&request_body) {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let text = response.body_mut().read_to_string().map_err(|e| format!("Couldn't read the model's reply: {e}"))?;
                if status == 200 {
                    let reply: Value = serde_json::from_str(&text).map_err(|e| format!("Unexpected reply from {base_url}: {e}"))?;
                    return from_chat(&reply);
                }
                let message = error_message(&text);
                match status {
                    400 if !renamed && message.contains("max_completion_tokens") => {
                        if let Some(n) = request_body.as_object_mut().and_then(|o| o.remove("max_tokens")) {
                            request_body["max_completion_tokens"] = n;
                        }
                        renamed = true;
                        continue;
                    }
                    401 | 403 => return Err(format!("{base_url} rejected the API key: check it in the assistant's settings ({message})")),
                    404 => return Err(format!("{base_url} doesn't know that model, or isn't a chat completions server: {message}")),
                    408 | 429 | 500 | 502 | 503 | 504 | 529 if attempt < 4 => {}
                    _ => return Err(format!("Model error {status}: {message}")),
                }
            }
            Err(e) if attempt < 4 && !is_local(base_url) => {
                let _ = e;
            }
            Err(e) => return Err(unreachable(base_url, e)),
        }
        std::thread::sleep(std::time::Duration::from_secs(wait));
        wait *= 2;
    }
    Err(format!("{base_url} is busy: try again in a minute"))
}

// The models a server offers (GET /models), sorted.
pub fn list_models(base_url: &str, key: &str) -> Result<Vec<String>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(20))).http_status_as_error(false).build().into();
    let mut request = agent.get(&endpoint(base_url, "models"));
    if !key.trim().is_empty() {
        request = request.header("authorization", &format!("Bearer {}", key.trim()));
    }
    let mut response = request.call().map_err(|e| unreachable(base_url, e))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("{base_url} answered {status}: {}", error_message(&text)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(|s| s.trim_start_matches("models/").to_owned())).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Err(format!("{base_url} listed no models"));
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anthropic_body() -> Value {
        json!({
            "model": "llama3.1",
            "max_tokens": 1000,
            "system": [{"type": "text", "text": "You build scenes.", "cache_control": {"type": "ephemeral"}}],
            "tools": [
                {"name": "edit_scene", "description": "edit", "input_schema": {"type": "object"}},
                {"name": "view_scene", "description": "look", "input_schema": {"type": "object"}}
            ],
            "messages": [
                {"role": "user", "content": "Make a box"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hm", "signature": "x"},
                    {"type": "text", "text": "Adding one."},
                    {"type": "tool_use", "id": "t1", "name": "edit_scene", "input": {"edits": []}},
                    {"type": "tool_use", "id": "t2", "name": "view_scene", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "made box1", "is_error": false},
                    {"type": "tool_result", "tool_use_id": "t2", "content": [
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
                        {"type": "text", "text": "Frame 1."}
                    ], "is_error": false}
                ]}
            ]
        })
    }

    #[test]
    fn requests_convert_to_chat_completions() {
        let r = chat_request(&anthropic_body(), true);
        let m = r["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({"role": "system", "content": "You build scenes."}));
        assert_eq!(m[1], json!({"role": "user", "content": "Make a box"}));
        // Thinking is dropped; text and calls stay, arguments as a JSON string.
        assert_eq!(m[2]["content"], "Adding one.");
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{\"edits\":[]}");
        assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "t1", "content": "made box1"}));
        assert!(m[4]["content"].as_str().unwrap().starts_with("Frame 1."));
        // The picture follows the tool results as a user message.
        assert_eq!(m[5]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        assert_eq!(r["tools"].as_array().unwrap().len(), 2);
        assert_eq!(r["tools"][0]["function"]["parameters"], json!({"type": "object"}));
        // Without vision: no view_scene tool and no images.
        let r = chat_request(&anthropic_body(), false);
        assert_eq!(r["tools"].as_array().unwrap().len(), 1);
        assert_eq!(r["messages"].as_array().unwrap().len(), 5);
        assert!(r["messages"][4]["content"].as_str().unwrap().contains("can't see images"));
    }

    #[test]
    fn replies_convert_back() {
        let reply = json!({
            "choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "On it.", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "edit_scene", "arguments": "{\"edits\":[{\"op\":\"create_object\"}]}"}},
                {"type": "function", "function": {"name": "inspect_node", "arguments": {"node": "box1"}}}
            ]}}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 30, "prompt_tokens_details": {"cached_tokens": 100}}
        });
        let r = from_chat(&reply).unwrap();
        assert_eq!(r["stop_reason"], "tool_use");
        assert_eq!(r["content"][0], json!({"type": "text", "text": "On it."}));
        assert_eq!(r["content"][1]["input"]["edits"][0]["op"], "create_object");
        assert_eq!(r["content"][2]["id"], "call_1");
        assert_eq!(r["content"][2]["input"]["node"], "box1");
        assert_eq!(r["usage"], json!({"input_tokens": 20, "cache_read_input_tokens": 100, "output_tokens": 30}));
        let done = from_chat(&json!({"choices": [{"finish_reason": "length", "message": {"content": "…"}}]})).unwrap();
        assert_eq!(done["stop_reason"], "max_tokens");
        assert!(from_chat(&json!({"error": "nope"})).is_err());
    }

    #[test]
    fn presets_and_hosts() {
        assert_eq!(preset_label("http://localhost:11434/v1/"), "Ollama (this computer)");
        assert_eq!(preset_label("https://example.com/v1"), "Custom server");
        assert!(is_local("http://localhost:1234/v1") && is_local("http://127.0.0.1:8080/v1") && is_local("http://studio.local:11434/v1"));
        assert!(!is_local("https://api.openai.com/v1"));
    }
}
