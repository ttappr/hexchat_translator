
#![allow(dead_code, unused_imports, unused_variables)]

//! This Hexchat addon provides commands that can turn on language translation
//! in any chat window of Hexhat. The user's text is translated to the target
//! language going out, and incoming message are translated back into the user's
//! own language. The user sees both the original text and translated text in
//! their Hexchat client, but other's in the channel only see the translated
//! text.
//!
//! The addon provides the following commands:
//! 
//! * `/LISTLANG` - Lists the names and 2 character codes for all the supported 
//!                 languages. The names or codes can be used to turn on 
//!                 translation with `/SETLANG`.
//! * `/SETLANG`  - Sets the source language (of the user) and the target 
//!                 language to translate to/from for the user.
//! * `/LSAY`     - Like `/SAY`, but performs translation. Required for
//!                 outgoing translations. Without using this command, the 
//!                 user's messages are sent normally. With the command they're
//!                 translated and sent to the channel.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::thread;
use std::rc::Rc;

//use std::sync::Arc;
//use std::sync::Mutex;

use hexchat_api::*;

use UserData::*;

dll_entry_points!(plugin_info, plugin_init, plugin_deinit);

type ChanData     = (String, String);
type ChanMap      = HashMap<ChanData, ChanData>;

/// Called when the plugin is loaded to register it with Hexchat.
///
fn plugin_info() -> PinnedPluginInfo {
    PluginInfo::new(
        "Language Translator",
        "0.1",
        "Instantly translated conversation in over 100 languages.")
}

/// Called when the plugin is loaded.
///
fn plugin_init(hc: &Hexchat) -> i32 {
    hc.print("Language Translator loaded");
    
    // `user_data` holds a `HashMap` that maps contexts, `(network, channel)`, 
    // to chosen translation, `(source_lang, target_lang)`. 
    let user_data  = UserData::shared(HashMap::<ChanData, ChanData>::new());
    
    let lsay_udata = UserData::boxed(("SAY", user_data.clone()));
    let lme_udata  = UserData::boxed(("ME", user_data.clone()));
    
    // Register the commands.
    
    hc.hook_command(
        "LISTLANG", Priority::Norm, on_cmd_listlang, LISTLANG_HELP, NoData);
        
    hc.hook_command(
        "SETLANG", Priority::Norm, on_cmd_setlang,   SETLANG_HELP, user_data
                                                                   .clone());
    hc.hook_command(
        "OFFLANG", Priority::Norm, on_cmd_offlang,   OFFLANG_HELP, user_data
                                                                   .clone());
    hc.hook_command(
        "LSAY",    Priority::Norm, on_cmd_lsay,      LSAY_HELP,    lsay_udata);

    hc.hook_command(
        "LME",     Priority::Norm, on_cmd_lsay,      LME_HELP,     lme_udata);


    // Register the handler for all the interesting text events.
    
    for event in &["Channel Message", "Channel Msg Hilight", 
                   "Channel Action",  "Channel Action Hilight", 
                   "Private Message", "Private Message to Dialog",
                   "Private Action",  "Private Action to Dialog", 
                   "You Part",        "You Part with Reason", 
                   "Disconnected"] 
    {
        let ud = UserData::boxed((*event, user_data.clone()));
        
        hc.hook_print(event, Priority::Norm, on_recv_message, ud);
    }

    1
}

/// Called when the plugin is unloaded.
///
fn plugin_deinit(hc: &Hexchat) -> i32 {
    hc.print("Language Translator unloaded");
    1
}


/// Returns Option((sourcelang, targetlang)) for the window receiving
/// an event. If there's no entry in the map, or there's a problem accessing it,
/// `None` is returned.
/// # Arguments
/// * `hc`        - The Hexchat interface.
/// * `user_data` - The user data of the invoking command.
/// # Returns
/// * Returns the channel data for the current context. This is obtained from
///   the `HashMap` that maps contexts to the source and dest languages.
///   If a context hasn't been set up for transation, `None` is returned.
///
fn get_channel_langs(hc        : &Hexchat, 
                     user_data : &UserData) -> Option<ChanData> 
{
    let network = hc.get_info("network")?;
    let channel = hc.get_info("channel")?;
    user_data.apply(
        |chan_map: &ChanMap| {
            if let Some(langs) = chan_map.get(&(network, channel)) {
                Some(langs.clone())
            } else { None }
        }).expect("User data downcast to &ChanMap failed.")
}

/// Activates the current context for language translation. A `HashMap` is
/// maintained that maps contexts (network/channel) to the desired translation
/// (source_lang, dest_lang).
/// # Arguments
/// * `hc`        - The Hexchat interface.
/// * `user_data` - The user data of the invoking command.
/// * `source`    - The source language to translate from.
/// * `dest`      - The destination language to translate to.
///
fn activate(hc        : &Hexchat, 
            user_data : &mut UserData, 
            source    : &str, 
            dest      : &str) 
{
    // TODO - The get_info() calls can fail when Hexchat isn't connected to a
    //        server. Or the active window isn't a channel. Need to handle
    //        these cases - or else get stack traces.
    let network = hc.get_info("network").expect("Unable to get network name.");
    let channel = hc.get_info("channel").expect("Unable to get channel name.");
    user_data.apply_mut(
        |chan_map: &mut ChanMap| {
            chan_map.insert((network, channel), 
                            (source.to_string(), dest.to_string()))
        }).expect("Activation failed.");
    // TODO - Should I make it a panic if the downcast fails? Otherwise, this
    //        fails silentely and other developers would have no idea it was.
}

/// Removes the current context's key and value from the `HashMap` that maps
/// active contexts to translation information (source-lang, dest-lang). This
/// effectively disables language translation in that window if it was 
/// on before. It has no effect if not.
///
fn deactivate(hc        : &Hexchat, 
              user_data : &mut UserData) 
{
    let network = hc.get_info("network").expect("Unable to get network name.");
    let channel = hc.get_info("channel").expect("Unable to get channel name.");
    user_data.apply_mut(
        |chan_map: &mut ChanMap| {
            chan_map.remove(&(network, channel))
        }).expect("Deactivation failed.");
}

/// Implements the /SETLANG command. Use /SETLANG to set the source and
/// target language for translation. Issuing this command activates 
/// the channel for translation.
///
fn on_cmd_setlang(hc        : &Hexchat, 
                  word      : &[String], 
                  word_eol  : &[String], 
                  user_data : &mut UserData
                 ) -> Eat 
{
    if word.len() == 3 {
        let mut src_lang = word[1].as_str();
        let mut tgt_lang = word[2].as_str();
        
        // Verify each lang is in the list below.
        if let Some(src_lang_info) = find_lang(src_lang) /* && */ {
        if let Some(tgt_lang_info) = find_lang(tgt_lang) {
        
            if src_lang_info !=  tgt_lang_info {
                    
                // Make sure the language names are the abbreviation.
                src_lang  =  src_lang_info.1;
                tgt_lang  =  tgt_lang_info.1;

                // Activate the channel.
                activate(hc, user_data, src_lang, tgt_lang);
                
                hc.print(&format!(
                         "TRANSLATION IS ON FOR THIS CHANNEL! \
                            {} (you) to {} (them).", src_lang_info.0, 
                                                     tgt_lang_info.0));
            } else {
                hc.print("BAD LANGUAGE PARAMETERS. Use /LISTLANG to \
                          get a list of supported languages. And don't \
                          set translation source and target languages the \
                          same.");
            }
        }}
    } else {
        hc.print(&format!("USAGE: {}", SETLANG_HELP));
    }
    Eat::All
}

/// Implements the /OFFLANG command. Turns translation off in the 
/// open window/channel.
///
fn on_cmd_offlang(hc        : &Hexchat, 
                  word      : &[String], 
                  word_eol  : &[String], 
                  user_data : &mut UserData
                 ) -> Eat 
{
    if word.len() == 1 {
        deactivate(hc, user_data);
        hc.print("Translation turned OFF for this channel.");
    } else {
        hc.print(&format!("USAGE: {}", OFFLANG_HELP));
    }
    Eat::All
}

/// mplements the /LSAY and /LME commands. Use /LSAY or /LME followed 
/// by whatever text you want. The text will be translated and posted to 
/// the channel. Other users will only see the translated message.
///
fn on_cmd_lsay(hc        : &Hexchat, 
               word      : &[String], 
               word_eol  : &[String], 
               user_data : &mut UserData
              ) -> Eat 
{
    // Unpackage the user data to get which command this is for (LSAY/LME),
    // and get the `UserData` with the `HashMap` in it.
    let (cmd, ref user_data) = user_data.apply(
                                    |ud: &(&str, UserData)| {
                                        (ud.0, ud.1.clone())
                                    })
                                    .expect("Couldn't downcast user data in \
                                             LSAY/LME!");
        
    //hc.print(&format!(">> word={:?}, word_eol={:?}", word, word_eol));
        
    if let Some(chan_langs) = get_channel_langs(hc, user_data) {
        let src_lang  = chan_langs.0;
        let tgt_lang  = chan_langs.1;
        let message   = word_eol[1].clone();
        let strip_msg = hc.strip(&message, StripFlags::StripBoth).unwrap();
        let network   = hc.get_info("network").unwrap();
        let channel   = hc.get_info("channel").unwrap();
        
        thread::spawn(move || {
            let msg;
            match google_translate_free(&strip_msg, &src_lang, &tgt_lang) {
                Ok(trans) => { msg = trans   },
                Err(_)    => { msg = message }
            }
            main_thread(move |hc| {
                if let Some(ctx) = hc.find_context(&network, &channel) {
                    ctx.command(&format!("{} {}", cmd, msg));
                } else {
                    // TODO - Review all the error handling and change the model
                    //        or make whatever fixes.
                    hc.print("Failed to get context.");
                }
            });
        });
        Eat::All
    } else {
        Eat::None
    }
}

/// Callback invoked when channel events like 'Channel Message' occur. 
/// If translation is on for the channel, this callback will have it 
/// translated and update the context window with translated message text.
///
fn on_recv_message(hc        : &Hexchat, 
                   word      : &[String], 
                   user_data : &mut UserData
                  ) -> Eat 
{
    use StripFlags::*;
    
    let (event, ref user_data) = user_data.apply(
                                    |ud: &(&str, UserData)| {
                                        (ud.0, ud.1.clone())
                                    })
                                    .expect("Couldn't downcast user data in \
                                             message receive handler!");
    
    if let Some(chan_langs) = get_channel_langs(hc, user_data) {
        if word.last().unwrap() == "~" {
            // To avoid recursion, this handler appends the "~" to the end of
            // each `emit_print()` it generates so it can be caught here.
            return Eat::None;
        }
        let sender    = word[0].clone();
        let message   = word[1].clone();
        let strip_msg = hc.strip(&message, StripBoth).unwrap();
        let msg_type  = event;
        let mode_char = if word.len() > 2 
                             { word[2].clone() } 
                        else { "".to_string() };
        let src_lang  = chan_langs.0;
        let tgt_lang  = chan_langs.1;
        let network   = hc.get_info("network").unwrap();
        let channel   = hc.get_info("channel").unwrap();
        
        thread::spawn(move || {
            let msg;
            let success;
            match google_translate_free(&strip_msg, &src_lang, &tgt_lang) {
                Ok(trans) => { msg = trans;           success = true;  },
                Err(_)    => { msg = message.clone(); success = false; }
            }
            main_thread(move |hc| {
                if let Some(ctx) = hc.find_context(&network, &channel) {
                    if !mode_char.is_empty() {
                        ctx.emit_print(
                            msg_type, &[&sender, &msg, &mode_char, "~"]);
                    } else {
                        ctx.emit_print(msg_type, &[&sender, &msg, "~"]);
                    }
                    if success {
                        ctx.print(&format!("\x0313{}", message));
                    } else {
                       ctx.print(
                            &format!("\x0313Channel Translator: error."));
                    }
                } else {
                    hc.print("Failed to get context.");
                }
            });
        });
        Eat::All
    } else {
        Eat::None
    }
}

/// Uses the free translation web service provided by Google to translate
/// a chat text message to the desired target language.
/// # Arguments
/// * `text`    - The text to translate.
/// * `source`  - The source language of the text.
/// * `target`  - The language to translate the text to.
/// # Returns
/// * A result where `Ok()` contains the translated text, and `Err()` indicates
///   the translation failed.
///
fn google_translate_free(text   : &str, 
                         source : &str, 
                         target : &str
                        ) -> Result<String, ()> 
{
    // Free (but limited) Google Translate request URI.
    let url = format!("https://translate.googleapis.com/translate_a/single\
                       ?client=gtx\
                       &sl={source_lang}\
                       &tl={target_lang}\
                       &dt=t&q={source_text}",          
                      source_lang = source,
                      target_lang = target,
                      source_text = urlparse::quote(text, b"").unwrap());
                      
    Ok(text.to_string())
                      
/*
    result = None
        
    let tr_rsp = Request::get(url, timeout=self._trans_resp_timeout);
        
    if tr_rsp.status_code == requests.codes.ok:
        tr_json = json.loads(tr_rsp.text)
        tr_text = tr_json["data"]["translations"][0]["translatedText"]
        
        result = (True, tr_text)
    else:
        try:
            tr_json = json.loads(tr_rsp.text)
            err = tr_json["error"]["message"]
                
        except (json.decoder.JSONDecodeError, KeyError):
            
            err = requests.status_codes._codes[tr_rsp.status_code][0]

        result = (False, "Google translate web service reported %s."
                         % err)
        
    return result
*/
}

/// Implements the /LISTLANG command - prints out a list of all languages 
/// that the translation web services support.
///
fn on_cmd_listlang(hc        : &Hexchat, 
                   word      : &[String], 
                   _word_eol : &[String], 
                   _userdata : &mut UserData
                  ) -> Eat 
{
    if word.len() == 1 {
        hc.print("");
        hc.print("------------------------ Supported Languages \
                  ------------------------");
        let langs = &SUPPORTED_LANGUAGES;
        
        for i in (0..langs.len()).step_by(3) {
            let (a, b) = langs[i];
            let (c, d) = langs[i + 1];
            let (e, f) = langs[i + 2];
            hc.print(
                &format!("{:-15}{:3}        {:-15}{:3}        {:-15}{:3}", 
                         a, b, c, d, e, f));
        }
        hc.print("");
    } else {
        hc.print("USAGE: ");
    }
    Eat::All
}

/// Finds and gives back a tuple (<long-name>, <abbrev>) from the supported 
/// languages list. This can be used to verify the languages the user requested
/// to see if they exist and can be used to interact with translation services.
/// # Arguments
/// * `lang` - This can be the name of the langauge, or the two character code
///            for the language.
/// # Returns
/// * If a match is found, a tuple is returned from the `SUPPORTED_LANGUAGES`
///   array. It will have the long name for the language and its two character
///   code. 
///
fn find_lang(lang: &str) -> Option<&(&str, &str)> {
    let lang = lang.to_lowercase();
    for lang_info in &SUPPORTED_LANGUAGES {
        if lang == lang_info.0.to_lowercase() || lang == lang_info.1 {
            return Some(lang_info);
        }
    }
    None
}

/// Help strings printed when the user requests /HELP on any of the commands 
/// this addon provides.

const LISTLANG_HELP: &str = "/LISTLANG - Lists languages supported and \
                             their abbrevations. This command takes no \
                             parameters.";
                             
const SETLANG_HELP : &str = "/SETLANG <src> <tgt> - Sets source and target \
                             languages for the channel.";
                             
const OFFLANG_HELP : &str = "/OFFLANG - Deactivates translation on the \
                             channel. This command takes no paramters.";
                             
const LSAY_HELP    : &str = "/LSAY <message> - Sends a translated message \
                             to the channel.";
                             
const LME_HELP     : &str = "/LME <message> - Sends a channel action \
                             message translated.";

/// A listing of all the supported langauges.

const SUPPORTED_LANGUAGES: [(&str, &str); 102] = [

    ("Afrikaans",      "af"), ("Hmong",        "hmn"), ("Polish",       "pl"),
    ("Albanian",       "sq"), ("Hungarian",     "hu"), ("Portuguese",   "pt"),
    ("Amharic",        "am"), ("Icelandic",     "is"), ("Punjabi",      "pa"),
    ("Arabic",         "ar"), ("Igbo",          "ig"), ("Romanian",     "ro"),
    ("Armenian",       "hy"), ("Indonesian",    "id"), ("Russian",      "ru"),
    ("Azeerbaijani",   "az"), ("Irish",         "ga"), ("Samoan",       "sm"),
    ("Basque",         "eu"), ("Italian",       "it"), ("Scots_Gaelic", "gd"),
    ("Belarusian",     "be"), ("Japanese",      "ja"), ("Serbian",      "sr"),
    ("Bengali",        "bn"), ("Javanese",      "jw"), ("Sesotho",      "st"),
    ("Bosnian",        "bs"), ("Kannada",       "kn"), ("Shona",        "sn"),
    ("Bulgarian",      "bg"), ("Kazakh",        "kk"), ("Sindhi",       "sd"),
    ("Catalan",        "ca"), ("Khmer",         "km"), ("Sinhala",      "si"),
    ("Cebuano",       "ceb"), ("Korean",        "ko"), ("Slovak",       "sk"),
    ("Corsican",       "co"), ("Kurdish",       "ku"), ("Slovenian",    "sl"),
    ("Croatian",       "hr"), ("Kyrgyz",        "ky"), ("Somali",       "so"),
    ("Czech",          "cs"), ("Lao",           "lo"), ("Spanish",      "es"),
    ("Danish",         "da"), ("Latin",         "la"), ("Sundanese",    "su"),
    ("Dutch",          "nl"), ("Latvian",       "lv"), ("Swahili",      "sw"),
    ("English",        "en"), ("Lithuanian",    "lt"), ("Swedish",      "sv"),
    ("Esperanto",      "eo"), ("Luxembourgish", "lb"), ("Tagalog",      "tl"),
    ("Estonian",       "et"), ("Macedonian",    "mk"), ("Tajik",        "tg"),
    ("Finnish",        "fi"), ("Malagasy",      "mg"), ("Tamil",        "ta"),
    ("French",         "fr"), ("Malay",         "ms"), ("Telugu",       "te"),
    ("Frisian",        "fy"), ("Malayalam",     "ml"), ("Thai",         "th"),
    ("Galician",       "gl"), ("Maltese",       "mt"), ("Turkish",      "tr"),
    ("Georgian",       "ka"), ("Maori",         "mi"), ("Ukrainian",    "uk"),
    ("German",         "de"), ("Marathi",       "mr"), ("Urdu",         "ur"),
    ("Greek",          "el"), ("Mongolian",     "mn"), ("Uzbek",        "uz"),
    ("Gujarati",       "gu"), ("Myanmar",       "my"), ("Vietnamese",   "vi"),
    ("Haitian_Creole", "ht"), ("Nepali",        "ne"), ("Welsh",        "cy"),
    ("Hausa",          "ha"), ("Norwegian",     "no"), ("Xhosa",        "xh"),
    ("Hawaiian",      "haw"), ("Nyanja",        "ny"), ("Yiddish",      "yi"),
    ("Hebrew",         "he"), ("Pashto",        "ps"), ("Yoruba",       "yo"),
    ("Hindi",          "hi"), ("Persian",       "fa"), ("Zulu",         "zu")];
    
    
    
