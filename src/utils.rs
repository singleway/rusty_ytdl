use regex::Regex;
use serde::{Deserialize, Serialize};
use urlencoding::decode;

use crate::info_extras::{get_author, get_chapters, get_dislikes, get_likes, get_storyboards};
use crate::{Embed, StringUtils, Thumbnail, VideoDetails, VideoQuality, VideoSearchOptions};
use crate::{VideoOptions, BASE_URL};

const VALID_QUERY_DOMAINS: &'static [&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "gaming.youtube.com",
];

const AGE_RESTRICTED_URLS: &'static [&str] = &[
    "support.google.com/youtube/?p=age_restrictions",
    "youtube.com/t/community_guidelines",
];

const AUDIO_ENCODING_RANKS: &'static [&str] = &["mp4a", "mp3", "vorbis", "aac", "opus", "flac"];
const VIDEO_ENCODING_RANKS: &'static [&str] = &[
    "mp4v",
    "avc1",
    "Sorenson H.283",
    "MPEG-4 Visual",
    "VP8",
    "VP9",
    "H.264",
];

pub fn get_cver(info: &serde_json::Value) -> &str {
    info.get("responseContext")
        .and_then(|x| x.get("serviceTrackingParams"))
        .unwrap()
        .as_array()
        .and_then(|x| {
            let index = x
                .iter()
                .position(|r| {
                    r.as_object()
                        .and_then(|c| Some(c.get("service").unwrap().as_str().unwrap() == "CSI"))
                        .unwrap()
                })
                .unwrap();
            x.into_iter()
                .nth(index)
                .unwrap()
                .as_object()
                .and_then(|x| {
                    let second_array = x.get("params").unwrap().as_array().unwrap();
                    let second_index = second_array
                        .iter()
                        .position(|r| {
                            r.as_object()
                                .and_then(|c| {
                                    Some(c.get("key").unwrap().as_str().unwrap() == "cver")
                                })
                                .unwrap()
                        })
                        .unwrap();
                    second_array
                        .into_iter()
                        .nth(second_index)
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .get("value")
                })
                .unwrap()
                .as_str()
        })
        .unwrap()
}

pub fn get_html5player(body: &str) -> Option<String> {
    let html5player_res = Regex::new(r#"<script\s+src="([^"]+)"(?:\s+type="text\\//javascript")?\s+name="player_ias\\//base"\s*>|"jsUrl":"([^"]+)""#).unwrap();
    let caps = html5player_res.captures(body).unwrap();
    match caps.get(2) {
        Some(caps) => Some(caps.as_str().to_string()),
        None => match caps.get(3) {
            Some(caps) => Some(caps.as_str().to_string()),
            None => Some(String::from("")),
        },
    }
}

pub fn parse_video_formats(
    info: &serde_json::Value,
    format_functions: Vec<String>,
) -> Option<Vec<serde_json::Value>> {
    if info.as_object()?.contains_key("streamingData") {
        let formats = info
            .as_object()?
            .get("streamingData")
            .and_then(|x| x.get("formats"))?
            .as_array()?;
        let adaptive_formats = info
            .as_object()?
            .get("streamingData")
            .and_then(|x| x.get("adaptiveFormats"))?
            .as_array()?;
        let mut formats = [&formats[..], &adaptive_formats[..]].concat();

        for i in 0..formats.len() {
            let format = &mut formats[i];
            format.as_object_mut().and_then(|x| {
                let new_url = set_download_url(&mut serde_json::json!(x), format_functions.clone());

                // Delete unnecessary cipher, signatureCipher
                x.remove("signatureCipher");
                x.remove("cipher");

                x.insert("url".to_string(), new_url);

                // Add Video metaData
                if x.contains_key("qualityLabel") {
                    x.insert("hasVideo".to_owned(), serde_json::Value::Bool(true));
                }

                if x.contains_key("audioBitrate") || x.contains_key("audioQuality") {
                    x.insert("hasAudio".to_owned(), serde_json::Value::Bool(true));
                }

                if x.contains_key("mimeType") {
                    let container_value_arr = x
                        .get("mimeType")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .split(";")
                        .collect::<Vec<&str>>()
                        .get(0)
                        .unwrap_or(&"")
                        .split("/")
                        .collect::<Vec<&str>>();

                    let container_value = container_value_arr.get(1).unwrap_or(&"");

                    if !container_value.is_empty() {
                        x.insert("container".to_owned(), serde_json::json!(container_value));
                    }

                    let codecs_value = between(
                        x.get("mimeType").and_then(|x| x.as_str()).unwrap_or(""),
                        r#"codecs=""#,
                        r#"""#,
                    );

                    if !codecs_value.is_empty() {
                        x.insert("codecs".to_string(), serde_json::json!(codecs_value));
                    }
                }

                if x.contains_key("hasVideo") && x.contains_key("codecs") {
                    let video_codec_value = x
                        .get("codecs")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .split(", ")
                        .collect::<Vec<&str>>()[0];

                    if !video_codec_value.is_empty() {
                        x.insert(
                            "videoCodec".to_string(),
                            serde_json::json!(video_codec_value),
                        );
                    }
                }

                if x.contains_key("hasAudio") && x.contains_key("codecs") {
                    let audio_codec_value_arr = x
                        .get("codecs")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .split(", ")
                        .collect::<Vec<&str>>();

                    let audio_codec_value = audio_codec_value_arr.last().unwrap_or(&"");

                    if !audio_codec_value.is_empty() {
                        x.insert(
                            "audioCodec".to_string(),
                            serde_json::json!(audio_codec_value),
                        );
                    }
                }

                let regex_is_live = Regex::new(r"\bsource[/=]yt_live_broadcast\b").unwrap();
                let regex_is_hls = Regex::new(r"/manifest/hls_(variant|playlist)/").unwrap();
                let regex_is_dashmpd = Regex::new(r"/manifest/dash/").unwrap();

                x.insert(
                    "isLive".to_string(),
                    serde_json::Value::Bool(
                        regex_is_live.is_match(x.get("url").and_then(|x| x.as_str()).unwrap_or("")),
                    ),
                );

                x.insert(
                    "isHLS".to_string(),
                    serde_json::Value::Bool(
                        regex_is_hls.is_match(x.get("url").and_then(|x| x.as_str()).unwrap_or("")),
                    ),
                );

                x.insert(
                    "isDashMPD".to_string(),
                    serde_json::Value::Bool(
                        regex_is_dashmpd
                            .is_match(x.get("url").and_then(|x| x.as_str()).unwrap_or("")),
                    ),
                );

                Some(x)
            });
        }

        Some(formats)
    } else {
        None
    }
}

pub fn filter_formats<'a>(
    formats: &'a Vec<serde_json::Value>,
    options: &'a VideoOptions,
) -> Vec<&'a serde_json::Value> {
    match options.filter {
        VideoSearchOptions::Audio => {
            return formats
                .iter()
                .filter(|x| x.get("hasVideo").is_none() && x.get("hasAudio").is_some())
                .collect::<Vec<&serde_json::Value>>();
        }
        VideoSearchOptions::Video => {
            return formats
                .iter()
                .filter(|x| x.get("hasVideo").is_some() && x.get("hasAudio").is_none())
                .collect::<Vec<&serde_json::Value>>();
        }
        _ => {
            return formats
                .iter()
                .filter(|x| x.get("hasVideo").is_some() && x.get("hasAudio").is_some())
                .collect::<Vec<&serde_json::Value>>();
        }
    }
}

pub fn choose_format<'a>(
    formats: &'a Vec<serde_json::Value>,
    options: &'a VideoOptions,
) -> &'a serde_json::Value {
    let mut return_formats = filter_formats(formats, options);

    if return_formats.iter().any(|x| {
        x.get("isHLS").is_some() && x.get("isHLS").and_then(|x| x.as_bool()).unwrap_or(false)
    }) {
        return_formats = formats
            .iter()
            .filter(|fmt| {
                (fmt.get("isHLS").is_some()
                    && fmt.get("isHLS").and_then(|x| x.as_bool()).unwrap_or(false))
                    || !(fmt.get("isLive").is_some()
                        && fmt.get("isLive").and_then(|x| x.as_bool()).unwrap_or(false))
            })
            .collect::<Vec<&serde_json::Value>>();
    }

    // Sort formats
    return_formats.sort_by(|a, b| sort_formats(a, b));

    match options.quality {
        VideoQuality::Highest => {
            return return_formats[0];
        }
        VideoQuality::Lowest => {
            return return_formats[return_formats.len() - 1];
        }
    }
}

pub fn sort_formats_by<F>(
    a: &serde_json::Value,
    b: &serde_json::Value,
    sort_by: Vec<F>,
) -> std::cmp::Ordering
where
    F: FnMut(&serde_json::Value) -> i32,
{
    let mut res = 0;

    for mut func in sort_by {
        res = func(b) - func(a);

        if res != 0 {
            break;
        }
    }

    // return res;
    if res >= 1 {
        return std::cmp::Ordering::Greater;
    } else if res < 0 {
        return std::cmp::Ordering::Less;
    } else {
        return std::cmp::Ordering::Equal;
    }
}

pub fn sort_formats(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    sort_formats_by(
        a,
        b,
        [
            // Formats with both video and audio are ranked highest.
            |form: &serde_json::Value| {
                form.get("isHLS").and_then(|x| x.as_bool()).unwrap_or(false) as i32
            },
            |form: &serde_json::Value| {
                form.get("isDashMPD")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false) as i32
            },
            |form: &serde_json::Value| {
                (form
                    .get("contentLength")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0)
                    > 0) as i32
            },
            |form: &serde_json::Value| {
                (form
                    .get("hasVideo")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
                    && form
                        .get("hasAudio")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false)) as i32
            },
            |form: &serde_json::Value| {
                form.get("hasVideo")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false) as i32
            },
            |form: &serde_json::Value| {
                let mut quality_label = form
                    .get("qualityLabel")
                    .and_then(|x| x.as_str())
                    .unwrap_or("0p")
                    .to_string();

                // Remove "p" char
                quality_label.pop();

                quality_label.parse().unwrap_or(0)
            },
            |form: &serde_json::Value| {
                form.get("bitrate").and_then(|x| x.as_i64()).unwrap_or(0) as i32
            },
            |form: &serde_json::Value| {
                form.get("audioBitrate")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0) as i32
            },
            |form: &serde_json::Value| {
                form.get("audioBitrate")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0) as i32
            },
            // getVideoEncodingRank,
            |form: &serde_json::Value| {
                let index = VIDEO_ENCODING_RANKS
                    .iter()
                    .position(|enc| {
                        form.get("codecs").is_some()
                            && form
                                .get("codecs")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .contains(enc)
                    })
                    .unwrap_or(usize::MAX);

                if index == usize::MAX {
                    return -1;
                }

                return index as i32;
            },
            // getAudioEncodingRank,
            |form: &serde_json::Value| {
                let index = AUDIO_ENCODING_RANKS
                    .iter()
                    .position(|enc| {
                        form.get("codecs").is_some()
                            && form
                                .get("codecs")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .contains(enc)
                    })
                    .unwrap_or(usize::MAX);

                if index == usize::MAX {
                    return -1;
                }

                return index as i32;
            },
        ]
        .to_vec(),
    )
}

pub fn set_download_url(
    format: &mut serde_json::Value,
    functions: Vec<String>,
) -> serde_json::Value {
    let empty_string_serde_value = serde_json::json!("");
    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Query {
        n: String,
        url: String,
        s: String,
        sp: String,
    }

    let empty_script = "".to_string();
    let decipher_script_string = functions.get(0).unwrap_or(&empty_script);
    let n_transform_script_string = functions.get(1).unwrap_or(&empty_script);

    fn decipher(url: &str, decipher_script_string: &str) -> String {
        let args: serde_json::value::Map<String, serde_json::Value> =
            serde_qs::from_str(url).unwrap();

        if args.get("s").is_none() || decipher_script_string.is_empty() {
            if args.get("url").is_none() {
                return url.to_string();
            } else {
                let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
                return args_url.to_string();
            }
        }

        let decipher_script = js_sandbox::Script::from_string(decipher_script_string);

        if decipher_script.is_err() {
            if args.get("url").is_none() {
                return url.to_string();
            } else {
                let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
                return args_url.to_string();
            }
        }

        let result = decipher_script
            .unwrap()
            .call("Wxa", &args.get("s").and_then(|x| x.as_str()).unwrap_or(""));

        if result.is_err() {
            if args.get("url").is_none() {
                return url.to_string();
            } else {
                let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
                return args_url.to_string();
            }
        }

        let result: String = result.unwrap();

        let return_url = url::Url::parse(args.get("url").and_then(|x| x.as_str()).unwrap_or(""));

        if return_url.is_err() {
            if args.get("url").is_none() {
                return url.to_string();
            } else {
                let args_url = args.get("url").and_then(|x| x.as_str()).unwrap_or("");
                return args_url.to_string();
            }
        }

        let mut return_url = return_url.unwrap();

        let query_name = if args.get("sp").is_some() {
            args.get("sp")
                .and_then(|x| x.as_str())
                .unwrap_or("signature")
        } else {
            "signature"
        };

        let mut query = return_url
            .query_pairs()
            .map(|(name, value)| {
                if name == query_name {
                    (name.into_owned(), result.to_string())
                } else {
                    (name.into_owned(), value.into_owned())
                }
            })
            .collect::<Vec<(String, String)>>();

        if !return_url.query_pairs().any(|(name, _)| name == query_name) {
            query.push((query_name.to_string(), result.to_string()));
        }

        return_url.query_pairs_mut().clear().extend_pairs(&query);

        return_url.to_string()
    }

    fn ncode(url: &str, n_transform_script_string: &str) -> String {
        let components: serde_json::value::Map<String, serde_json::Value> =
            serde_qs::from_str(&decode(url).unwrap_or(std::borrow::Cow::Borrowed(url))).unwrap();

        if components.get("n").is_none() || n_transform_script_string.is_empty() {
            return url.to_string();
        }

        let n_transform_script = js_sandbox::Script::from_string(n_transform_script_string);

        if n_transform_script.is_err() {
            return url.to_string();
        }

        let result = n_transform_script.unwrap().call(
            "pla",
            &components.get("n").and_then(|x| x.as_str()).unwrap_or(""),
        );

        if result.is_err() {
            return url.to_string();
        }

        let result: String = result.unwrap();

        // println!(
        //     "{:?} {:?}",
        //     components.get("n").and_then(|x| x.as_str()).unwrap_or(""),
        //     result
        // );

        let return_url = url::Url::parse(url);

        if return_url.is_err() {
            return url.to_string();
        }

        let mut return_url = return_url.unwrap();

        let query = return_url
            .query_pairs()
            .map(|(name, value)| {
                if name == "n" {
                    (name.into_owned(), result.to_string())
                } else {
                    (name.into_owned(), value.into_owned())
                }
            })
            .collect::<Vec<(String, String)>>();

        return_url.query_pairs_mut().clear().extend_pairs(&query);

        return_url.to_string()
    }

    let return_format = format.as_object_mut().unwrap();

    let cipher = return_format.get("url").is_none();
    let url = return_format
        .get("url")
        .unwrap_or(
            return_format.get("signatureCipher").unwrap_or(
                return_format
                    .get("cipher")
                    .unwrap_or(&empty_string_serde_value),
            ),
        )
        .as_str()
        .unwrap_or("");

    if cipher {
        return_format.insert(
            "url".to_string(),
            serde_json::json!(&ncode(
                decipher(url, decipher_script_string.as_str()).as_str(),
                n_transform_script_string.as_str()
            )),
        );
    } else {
        return_format.insert(
            "url".to_string(),
            serde_json::json!(&ncode(url, n_transform_script_string.as_str())),
        );
    }

    // Delete unnecessary cipher, signatureCipher
    return_format.remove("signatureCipher");
    return_format.remove("cipher");

    let return_url = url::Url::parse(
        return_format
            .get("url")
            .and_then(|x| x.as_str())
            .unwrap_or(""),
    )
    .unwrap();

    serde_json::json!(return_url.to_string())
}

pub fn get_video_id(url: &str) -> Option<String> {
    let url_regex = Regex::new(r"^https?://").unwrap();

    if validate_id(url.to_string()) {
        return Some(url.to_string());
    } else if url_regex.is_match(url.trim()) {
        let id = get_url_video_id(url);

        if id.is_none() {
            return None;
        }

        return Some(id.unwrap());
    } else {
        None
    }
}

pub fn validate_id(id: String) -> bool {
    let id_regex = Regex::new(r"^[a-zA-Z0-9-_]{11}$").unwrap();

    id_regex.is_match(id.trim())
}

fn get_url_video_id(url: &str) -> Option<String> {
    let valid_path_domains =
        Regex::new(r"^https?:\\//\\//(youtu\.be\\//|(www\.)?youtube\.com\\//(embed|v|shorts)\\//)")
            .unwrap();

    let parsed_result = url::Url::parse(url.trim());

    if parsed_result.is_err() {
        return None;
    }

    let parsed = url::Url::parse(url.trim()).unwrap();

    let mut id: Option<String> = None;

    for value in parsed.query_pairs() {
        if value.0.to_string().as_str() == "v" {
            id = Some(value.1.to_string());
        }
    }

    if valid_path_domains.is_match(url.trim()) && id.is_none() {
        let paths = parsed.host_str().unwrap_or_else(|| "").split("/");
        if url::Url::parse(url.trim())
            .unwrap()
            .host_str()
            .unwrap_or_else(|| "")
            == "youtu.be"
        {
            id = Some(
                paths
                    .collect::<Vec<&str>>()
                    .get(1)
                    .unwrap_or_else(|| &"")
                    .to_string(),
            );
        } else {
            id = Some(
                paths
                    .collect::<Vec<&str>>()
                    .get(2)
                    .unwrap_or_else(|| &"")
                    .to_string(),
            );
        }
    } else if url::Url::parse(url.trim()).unwrap().host_str().is_some()
        && !VALID_QUERY_DOMAINS
            .iter()
            .any(|domain| domain == &parsed.host_str().unwrap_or_else(|| ""))
    {
        return None;
    }

    if id.is_none() {
        return None;
    }

    id = Some(id.unwrap().substring(0, 11).to_string());

    if !validate_id(id.clone().unwrap()) {
        return None;
    }
    return id;
}

pub fn get_text(obj: &serde_json::Value) -> &serde_json::Value {
    let null_referance = &serde_json::Value::Null;
    obj.as_object()
        .and_then(|x| {
            if x.contains_key("runs") {
                x.get("runs").and_then(|c| {
                    c.as_array()
                        .unwrap()
                        .get(0)
                        .and_then(|d| d.as_object().and_then(|f| f.get("text")))
                })
            } else {
                x.get("simpleText")
            }
        })
        .unwrap_or_else(|| null_referance)
}

pub fn clean_video_details(
    initial_response: &serde_json::Value,
    player_response: &serde_json::Value,
    media: serde_json::Value,
    id: String,
) -> VideoDetails {
    let empty_serde_object = serde_json::json!({});
    let empty_serde_vec: Vec<serde_json::Value> = vec![];
    let empty_serde_map = serde_json::Map::new();

    let mut data = player_response
        .get("microformat")
        .and_then(|x| x.get("playerMicroformatRenderer"))
        .unwrap_or_else(|| &empty_serde_object)
        .clone();
    let player_response_video_details = player_response
        .get("videoDetails")
        .unwrap_or_else(|| &empty_serde_object)
        .clone();

    // merge two json objects
    merge(&mut data, &player_response_video_details);

    let embed_object = data
        .get("embed")
        .and_then(|x| x.as_object())
        .unwrap_or_else(|| &empty_serde_map);
    VideoDetails {
        author: get_author(&initial_response, &player_response).unwrap(),
        age_restricted: is_age_restricted(&media),

        media,
        likes: get_likes(&initial_response),
        dislikes: get_dislikes(&initial_response),

        video_url: format!("{BASE}{ID}", BASE = BASE_URL, ID = id),
        storyboards: get_storyboards(&player_response).unwrap(),
        chapters: get_chapters(&initial_response).unwrap(),

        embed: Embed {
            flash_secure_url: embed_object
                .get("flashSecureUrl")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| "")
                .to_string(),
            flash_url: embed_object
                .get("flashUrl")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| "")
                .to_string(),
            iframe_url: embed_object
                .get("iframeUrl")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| "")
                .to_string(),
            height: embed_object
                .get("height")
                .and_then(|x| {
                    if x.is_string() {
                        x.as_str().and_then(|x| match x.parse::<i64>() {
                            Ok(a) => Some(a),
                            Err(_err) => Some(0i64),
                        })
                    } else {
                        x.as_i64()
                    }
                })
                .unwrap_or_else(|| 0i64) as i32,
            width: embed_object
                .get("width")
                .and_then(|x| {
                    if x.is_string() {
                        x.as_str().and_then(|x| match x.parse::<i64>() {
                            Ok(a) => Some(a),
                            Err(_err) => Some(0i64),
                        })
                    } else {
                        x.as_i64()
                    }
                })
                .unwrap_or_else(|| 0i64) as i32,
        },
        title: data
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        description: if data.get("shortDescription").is_some() {
            data.get("shortDescription")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| "")
                .to_string()
        } else {
            get_text(
                data.get("description")
                    .unwrap_or_else(|| &empty_serde_object),
            )
            .as_str()
            .unwrap_or_else(|| "")
            .to_string()
        },
        length_seconds: data
            .get("lengthSeconds")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "0")
            .to_string(),
        owner_profile_url: data
            .get("ownerProfileUrl")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        external_channel_id: data
            .get("externalChannelId")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        is_family_safe: data
            .get("isFamilySafe")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        available_countries: data
            .get("availableCountries")
            .and_then(|x| x.as_array())
            .unwrap_or_else(|| &empty_serde_vec)
            .iter()
            .map(|x| x.as_str().unwrap_or_else(|| "").to_string())
            .collect::<Vec<String>>(),
        is_unlisted: data
            .get("isUnlisted")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        has_ypc_metadata: data
            .get("hasYpcMetadata")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        view_count: data
            .get("viewCount")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "0")
            .to_string(),
        category: data
            .get("category")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        publish_date: data
            .get("publishDate")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        owner_channel_name: data
            .get("ownerChannelName")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        upload_date: data
            .get("uploadDate")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        video_id: data
            .get("videoId")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "0")
            .to_string(),
        keywords: data
            .get("keywords")
            .and_then(|x| x.as_array())
            .unwrap_or_else(|| &empty_serde_vec)
            .iter()
            .map(|x| x.as_str().unwrap_or_else(|| "").to_string())
            .collect::<Vec<String>>(),
        channel_id: data
            .get("channelId")
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| "")
            .to_string(),
        is_owner_viewing: data
            .get("isOwnerViewing")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        is_crawlable: data
            .get("isCrawlable")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        allow_ratings: data
            .get("allowRatings")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        is_private: data
            .get("isPrivate")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        is_unplugged_corpus: data
            .get("isUnpluggedCorpus")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        is_live_content: data
            .get("isLiveContent")
            .and_then(|x| x.as_bool())
            .unwrap_or_else(|| false),
        thumbnails: data
            .get("thumbnail")
            .and_then(|x| x.get("thumbnails"))
            .and_then(|x| x.as_array())
            .unwrap_or_else(|| &empty_serde_vec)
            .iter()
            .map(|x| Thumbnail {
                width: x
                    .get("width")
                    .and_then(|x| {
                        if x.is_string() {
                            x.as_str().and_then(|x| match x.parse::<i64>() {
                                Ok(a) => Some(a),
                                Err(_err) => Some(0i64),
                            })
                        } else {
                            x.as_i64()
                        }
                    })
                    .unwrap_or_else(|| 0i64) as i32,
                height: x
                    .get("height")
                    .and_then(|x| {
                        if x.is_string() {
                            x.as_str().and_then(|x| match x.parse::<i64>() {
                                Ok(a) => Some(a),
                                Err(_err) => Some(0i64),
                            })
                        } else {
                            x.as_i64()
                        }
                    })
                    .unwrap_or_else(|| 0i64) as i32,
                url: x
                    .get("url")
                    .and_then(|x| x.as_str())
                    .unwrap_or_else(|| "")
                    .to_string(),
            })
            .collect::<Vec<Thumbnail>>(),
    }
}

pub fn is_verified(badges: &serde_json::Value) -> bool {
    badges
        .as_array()
        .and_then(|x| {
            let verified_index = x
                .iter()
                .position(|c| {
                    let json = serde_json::json!(c);
                    json["metadataBadgeRenderer"]["tooltip"] == "Verified"
                })
                .unwrap_or_else(|| usize::MAX);

            Some(verified_index < usize::MAX)
        })
        .unwrap_or_else(|| false)
}

pub fn is_age_restricted(media: &serde_json::Value) -> bool {
    let mut age_restricted = false;
    if media.is_object() && media.as_object().is_some() {
        age_restricted = AGE_RESTRICTED_URLS.iter().any(|url| {
            media
                .as_object()
                .and_then(|x| {
                    let mut bool_vec: Vec<bool> = vec![];

                    for (_key, value) in x {
                        if !value.is_string() {
                            bool_vec.push(false);
                        } else {
                            value
                                .as_str()
                                .and_then(|c| Some(bool_vec.push(c.contains(url))))
                                .unwrap_or_else(|| bool_vec.push(false));
                        }
                    }

                    Some(bool_vec.iter().any(|v| v == &true))
                })
                .unwrap_or_else(|| false)
        })
    }

    age_restricted
}

pub fn is_rental(player_response: &serde_json::Value) -> bool {
    let playability = player_response.get("playabilityStatus");

    if playability.is_none() {
        return false;
    }

    return playability
        .and_then(|x| x.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| "")
        == "UNPLAYABLE"
        && playability
            .and_then(|x| x.get("errorScreen"))
            .and_then(|x| x.get("playerLegacyDesktopYpcOfferRenderer"))
            .is_some();
}

pub fn is_not_yet_broadcasted(player_response: &serde_json::Value) -> bool {
    let playability = player_response.get("playabilityStatus");

    if playability.is_none() {
        return false;
    }

    return playability
        .and_then(|x| x.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        == "LIVE_STREAM_OFFLINE";
}

pub fn is_play_error(player_response: &serde_json::Value, statuses: Vec<&str>) -> bool {
    let playability = player_response
        .get("playabilityStatus")
        .and_then(|x| x.get("status").and_then(|x| x.as_str()));

    if playability.is_some() {
        return statuses.contains(&playability.unwrap());
    }

    return false;
}

pub fn is_private_video(player_response: &serde_json::Value) -> bool {
    if player_response
        .get("playabilityStatus")
        .and_then(|x| x.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        == "LOGIN_REQUIRED"
    {
        return true;
    }

    return false;
}

pub async fn get_functions(html5player: &str) -> Vec<String> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("https://www.youtube.com/{}", html5player))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    extract_functions(response)
}

pub fn extract_functions(body: String) -> Vec<String> {
    let mut functions: Vec<String> = vec![];
    fn extract_manipulations(body: String, caller: &str) -> String {
        let function_name = between(caller, r#"a=a.split("");"#, ".");
        if function_name.len() <= 0 {
            return String::new();
        }

        let function_start = format!(r#"var {function_name}={{"#, function_name = function_name);
        let ndx = body.find(function_start.as_str());

        if ndx.is_none() {
            return String::new();
        }

        let sub_body = body.slice((ndx.unwrap() + function_start.len() - 1)..);

        let return_formatted_string = format!(
            "var {function_name}={after_sub_body}",
            function_name = function_name,
            after_sub_body = cut_after_js(sub_body).unwrap_or(String::from("null")),
        );

        return return_formatted_string;
    }

    fn extract_decipher(body: String, functions: &mut Vec<String>) {
        let function_name = between(body.as_str(), r#"a.set("alr","yes");c&&(c="#, "(decodeURIC");

        if function_name.len() > 0 {
            let function_start =
                format!("{function_name}=function(a)", function_name = function_name);
            let ndx = body.find(function_start.as_str());

            if ndx.is_some() {
                let sub_body = body.slice((ndx.unwrap() + function_start.len())..);
                let mut function_body = format!(
                    "var {function_start}{cut_after_js_sub_body}",
                    function_start = function_start,
                    cut_after_js_sub_body = cut_after_js(sub_body).unwrap_or(String::from("{}"))
                );

                function_body = format!(
                    "{manipulated_body};{function_body};",
                    manipulated_body = extract_manipulations(body.clone(), function_body.as_str()),
                    function_body = function_body,
                );

                function_body.retain(|c| c != '\n');

                functions.push(function_body);
            }
        }
    }

    fn extract_ncode(body: String, functions: &mut Vec<String>) {
        let mut function_name = between(body.as_str(), r#"&&(b=a.get("n"))&&(b="#, "(b)");
        let left_name = format!(
            "{splitted_function_name}=[",
            splitted_function_name = function_name
                .split("[")
                .collect::<Vec<&str>>()
                .get(0)
                .unwrap_or(&"")
        );

        if function_name.contains("[") {
            function_name = between(body.as_str(), left_name.as_str(), "]");
        }

        if function_name.len() > 0 {
            let function_start =
                format!("{function_name}=function(a)", function_name = function_name);
            let ndx = body.find(function_start.as_str());

            if ndx.is_some() {
                let sub_body = body.slice((ndx.unwrap() + function_start.len())..);

                let mut function_body = format!(
                    "var {function_start}{cut_after_sub_body};",
                    function_start = function_start,
                    cut_after_sub_body = cut_after_js(sub_body).unwrap_or(String::from("{}")),
                );

                function_body.retain(|c| c != '\n');

                functions.push(function_body);
            }
        }
    }
    extract_decipher(body.clone(), &mut functions);
    extract_ncode(body, &mut functions);

    // println!("{:#?} {}", functions, functions.len());
    functions
}

pub fn time_to_ms(duration: &str) -> usize {
    let mut ms = 0;
    for (i, curr) in duration.split(':').into_iter().rev().enumerate() {
        ms = ms
            + (curr.parse::<usize>().unwrap_or_else(|_e| 0)
                * (u32::pow(60 as u32, i as u32) as usize));
    }
    ms = ms * 1000;
    ms
}

pub fn parse_abbreviated_number(time_str: &str) -> usize {
    let replaced_string = time_str.replace(",", ".").replace(" ", "");
    let string_match_regex = Regex::new(r"([\d,.]+)([MK]?)").unwrap();
    // let mut return_value = 0usize;

    let caps = string_match_regex
        .captures(replaced_string.as_str())
        .unwrap();

    let return_value = if caps.len() > 0 {
        let mut num;
        let multi;

        match caps.get(1) {
            Some(regex_match) => {
                num = regex_match
                    .as_str()
                    .parse::<f32>()
                    .unwrap_or_else(|_x| 0f32)
            }
            None => num = 0f32,
        }

        match caps.get(2) {
            Some(regex_match) => multi = regex_match.as_str(),
            None => multi = "",
        }

        match multi {
            "M" => num = num * 1000000f32,
            "K" => num = num * 1000f32,
            _ => num = num,
        }

        num = num.round();
        num as usize
    } else {
        0usize
    };

    return_value
}

pub fn merge(a: &mut serde_json::Value, b: &serde_json::Value) {
    match (a, b) {
        (&mut serde_json::Value::Object(ref mut a), &serde_json::Value::Object(ref b)) => {
            for (k, v) in b {
                merge(a.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => {
            *a = b.clone();
        }
    }
}

pub fn between<'a>(haystack: &'a str, left: &'a str, right: &'a str) -> &'a str {
    let left_index = haystack.find(left);
    if left_index.is_none() {
        return "";
    }

    let mut pos = left_index.unwrap();
    pos += left.len();

    let mut return_str = haystack.slice(pos..);
    let right_index = return_str.find(right);
    if right_index.is_none() {
        return "";
    }

    let second_pos = right_index.unwrap();

    return_str = return_str.substring(0, second_pos);
    return_str
}

pub fn cut_after_js(mixed_json: &str) -> Option<String> {
    let escaping_sequenzes = serde_json::json!([
      { "start": '"', "end": '"' },
      { "start": "'", "end": "'" },
      { "start": "`", "end": "`" },
      { "start": "/", "end": "/", "startPrefix": r"(^|[\[{:;,])\s?$"},
    ]);

    let mut open = String::new();
    let mut close = String::new();

    if mixed_json.chars().nth(0).unwrap_or('\0') == '[' {
        open = "[".to_string();
        close = "]".to_string();
    } else if mixed_json.chars().nth(0).unwrap_or('\0') == '{' {
        open = "{".to_string();
        close = "}".to_string();
    }

    if open.is_empty() {
        return None;
    }

    let mut is_escaped_object: Option<&serde_json::Value> = None;

    // States if the current character is treated as escaped or not
    let mut is_escaped = false;

    // Current open brackets to be closed
    let mut counter = 0;

    let mut return_string: Option<String> = None;

    for i in 0..mixed_json.len() {
        if !is_escaped
            && is_escaped_object.is_some()
            && mixed_json.chars().nth(i)
                == is_escaped_object
                    .and_then(|x| x.get("end"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .chars()
                    .nth(0)
        {
            is_escaped_object = None;
            continue;
        } else if !is_escaped && is_escaped_object.is_none() {
            for escaped in escaping_sequenzes.as_array().unwrap() {
                if mixed_json.chars().nth(i).unwrap_or('\0')
                    != escaped
                        .get("start")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .chars()
                        .nth(0)
                        .unwrap_or('1')
                {
                    continue;
                }

                if escaped.get("startPrefix").is_none() {
                    is_escaped_object = Some(escaped);
                    break;
                }

                let start_prefix_regex =
                    Regex::new(escaped.get("startPrefix").and_then(|x| x.as_str()).unwrap())
                        .unwrap();

                if start_prefix_regex.is_match(mixed_json.substring(i - 10, i)) {
                    is_escaped_object = Some(escaped);
                    break;
                }
            }
            if is_escaped_object.is_some() {
                continue;
            }
        }

        is_escaped = mixed_json.chars().nth(i).unwrap_or('\0') == '\\' && !is_escaped;

        if is_escaped_object.is_some() {
            continue;
        }

        if mixed_json.chars().nth(i).unwrap_or('\0') == open.chars().nth(0).unwrap_or('1') {
            counter = counter + 1;
        } else if mixed_json.chars().nth(i).unwrap_or('\0') == close.chars().nth(0).unwrap_or('1') {
            counter = counter - 1;
        }

        if counter == 0 {
            // Return the cut JSON
            return_string = Some(mixed_json.substring(0, i + 1).to_string());
            break;
        }
    }
    return_string
}