
//! This Hexchat addon provides commands that can turn on language translation
//! in any chat window of Hexhat. The user's text is translated to the target
//! language going out, and incoming message are translated back into the user's
//! own language. The user sees both the original text and translated text in
//! their Hexchat client, but other's in the channel only see the translated
//! text.
//!
//! # The addon provides the following commands
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
//! * `/LME`      - A translator version of the `/ME` command.
//! * `/OFFLANG`  - Turns translation off in the current window.
//!

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::thread;
use std::time::Duration;

use hexchat_api::*;
use StripFlags::*;
use UserData::*;

const TRANSLATION_SERVER_TIMEOUT: u64 = 5; // Seconds.

// Register the entry points of the plugin.
//
dll_entry_points!(plugin_info, plugin_init, plugin_deinit);

type ChanData = (String, String);
type ChanMap  = HashMap<ChanData, ChanData>;

/// Called when the plugin is loaded to register it with Hexchat.
///
fn plugin_info() -> PinnedPluginInfo {
    PluginInfo::new(
        "Language Translator",
        "0.1.1",
        "Instantly translated conversation in over 100 languages.")
}

/// Called when the plugin is loaded.
///
fn plugin_init(hc: &Hexchat) -> i32 {
    hc.print("Language Translator loaded");
    
    // `map_udata` holds a `HashMap` that maps contexts, `(network, channel)`, 
    // to chosen translation, `(source_lang, target_lang)`. 
    let map_udata  = UserData::shared(HashMap::<ChanData, ChanData>::new());
    
    let lsay_udata = UserData::boxed(("SAY", map_udata.clone()));
    let lme_udata  = UserData::boxed(("ME", map_udata.clone()));
    
    // Register the commands.
    
    hc.hook_command(
        "LISTLANG", Priority::Norm, on_cmd_listlang, LISTLANG_HELP, NoData);
        
    hc.hook_command(
        "SETLANG", Priority::Norm, on_cmd_setlang,   SETLANG_HELP, map_udata
                                                                   .clone());
    hc.hook_command(
        "OFFLANG", Priority::Norm, on_cmd_offlang,   OFFLANG_HELP, map_udata
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
        let event_udata = UserData::boxed((*event, map_udata.clone()));
        
        hc.hook_print(event, Priority::Norm, on_recv_message, event_udata);
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
/// * `map_udata` - The user data of the invoking command.
/// # Returns
/// * Returns the channel data for the current context. This is obtained from
///   the `HashMap` that maps contexts to the source and dest languages.
///   If a context hasn't been set up for transation, `None` is returned.
///
fn get_channel_langs(hc        : &Hexchat, 
                     map_udata : &UserData) -> Option<ChanData> 
{
    let network = hc.get_info("network")?;
    let channel = hc.get_info("channel")?;
    map_udata.apply(
        |chan_map: &ChanMap| {
            if let Some(langs) = chan_map.get(&(network, channel)) {
                Some(langs.clone())
            } else { None }
        })
}

/// Activates the current context for language translation. A `HashMap` is
/// maintained that maps contexts (network/channel) to the desired translation
/// (source_lang, dest_lang).
/// # Arguments
/// * `hc`        - The Hexchat interface.
/// * `map_udata` - The user data of the invoking command.
/// * `source`    - The source language to translate from.
/// * `dest`      - The destination language to translate to.
///
fn activate(hc        : &Hexchat, 
            map_udata : &mut UserData, 
            source    : &str, 
            dest      : &str) 
{
    // TODO - The get_info() calls can fail when Hexchat isn't connected to a
    //        server. Or the active window isn't a channel. Need to handle
    //        these cases - or else get stack traces.
    let network = hc.get_info("network").expect("Unable to get network name.");
    let channel = hc.get_info("channel").expect("Unable to get channel name.");
    map_udata.apply_mut(
        |chan_map: &mut ChanMap| {
            chan_map.insert((network, channel), 
                            (source.to_string(), dest.to_string()))
        });
}

/// Removes the current context's key and value from the `HashMap` that maps
/// active contexts to translation information (source-lang, dest-lang). This
/// effectively disables language translation in that window if it was 
/// on before. It has no effect if not.
///
fn deactivate(hc        : &Hexchat, 
              map_udata : &mut UserData) 
{
    let network = hc.get_info("network").expect("Unable to get network name.");
    let channel = hc.get_info("channel").expect("Unable to get channel name.");
    map_udata.apply_mut(
        |chan_map: &mut ChanMap| {
            chan_map.remove(&(network, channel))
        });
}

/// Implements the /SETLANG command. Use /SETLANG to set the source and
/// target language for translation. Issuing this command activates 
/// the channel for translation.
///
fn on_cmd_setlang(hc        : &Hexchat, 
                  word      : &[String], 
                  _word_eol : &[String], 
                  map_udata : &mut UserData
                 ) -> Eat 
{
    if word.len() == 3 {
        let mut src_lang = word[1].as_str();
        let mut tgt_lang = word[2].as_str();
        
        let mut params_good = false;
        
        // Verify each lang is in the list below.
        if let Some(src_lang_info) = find_lang(src_lang) /* && */ {
        if let Some(tgt_lang_info) = find_lang(tgt_lang) {
        
            if src_lang_info !=  tgt_lang_info {
                params_good = true;
                    
                // Make sure the language names are the abbreviation.
                src_lang  =  src_lang_info.1;
                tgt_lang  =  tgt_lang_info.1;

                // Activate the channel.
                activate(hc, map_udata, src_lang, tgt_lang);
                
                hc.print(&format!(
                         "\x0313TRANSLATION IS ON FOR THIS CHANNEL! \
                          {} (you) to {} (them).", src_lang_info.0, 
                                                   tgt_lang_info.0));
            } 
        }}
        if !params_good {
            hc.print("\x0313BAD LANGUAGE PARAMETERS. Use /LISTLANG to \
                      get a list of supported languages. And don't \
                      set translation source and target languages the \
                      same.");
        }
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
                  _word_eol : &[String], 
                  map_udata : &mut UserData
                 ) -> Eat 
{
    if word.len() == 1 {
        deactivate(hc, map_udata);
        hc.print("\x0313Translation turned OFF for this channel.");
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
               _word     : &[String], 
               word_eol  : &[String], 
               user_data : &mut UserData
              ) -> Eat 
{
    // Unpackage the user data to get which command this is for (LSAY/LME),
    // and get the `UserData` with the `HashMap` in it.
    let (cmd, ref map_udata) = user_data.apply(
                                    |ud: &(&str, UserData)| {
                                        (ud.0, ud.1.clone())
                                    });

    if let Some(chan_langs) = get_channel_langs(hc, map_udata) {
        if { || {
            let src_lang  = chan_langs.0;
            let tgt_lang  = chan_langs.1;
            let message   = word_eol[1].clone();
            
            let strip_msg = hc.strip(&message, StripBoth)?;
            let network   = hc.get_info("network")?;                              
            let channel   = hc.get_info("channel")?;
            
            thread::spawn(move || {
                let msg;
                let mut emsg = None;
                let mut is_over_limit = false;
                
                match google_translate_free(&strip_msg, &src_lang, &tgt_lang) {
                    Ok(trans) => { 
                        msg  = trans;
                    },
                    Err(err)  => { 
                        msg  = err.get_partial_trans().to_string();
                        emsg = Some(format!("\x0313{}", err));
                        is_over_limit = err.is_over_limit();
                    }
                }
                main_thread(move |hc| -> Result<(), ContextError> {
                    if let Some(ctx) = hc.find_context(&network, &channel) {
                        ctx.command(&format!("{} {}", cmd, msg))?;
                        ctx.print(&format!("\x0311{}", message))?;
                           
                        if let Some(emsg) = &emsg {
                            ctx.print(&emsg)?;
                               
                            if is_over_limit {
                                ctx.command("OFFLANG")?;
                            }
                        }
                    } else {
                        hc.print("\x0313Failed to get context.");
                    }
                    Ok(())
                });
            });
            Some(())
        }}().is_none() {
            // If we get here, either `strip()` or `get_info()` returned None.
            hc.print("\x0313\
                     Translator Error: Basic failure retrieving channel \
                     information, or unable to strip original message.");
        }
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
    if word.len() < 2  || word.last().unwrap() == "~" {
        // To avoid recursion, this handler appends the "~" to the end of
        // each `emit_print()` it generates so it can be caught here.
        return Eat::None;
    }
    let (event, ref map_udata) = user_data.apply(
                                    |ud: &(&str, UserData)| {
                                        (ud.0, ud.1.clone())
                                    });
                                    
    if let Some(chan_langs) = get_channel_langs(hc, map_udata) {
        if { || {
            let sender    = word[0].clone();
            let message   = word[1].clone();
            let strip_msg = hc.strip(&message, StripBoth)?;
            let msg_type  = event;
            let mode_char = if word.len() > 2 
                                 { word[2].clone() } 
                            else { "".to_string() };
            let src_lang  = chan_langs.0;
            let tgt_lang  = chan_langs.1;
            
            let network   = hc.get_info("network")?;
            let channel   = hc.get_info("channel")?;
            
            thread::spawn(move || {
                let msg;
                let mut emsg = None;
                let mut is_over_limit = false;
                
                match google_translate_free(&strip_msg, &tgt_lang, &src_lang) {
                    Ok(trans) => { 
                        msg = trans;
                    },
                    Err(err)  => { 
                        msg  = err.get_partial_trans().to_string();
                        emsg = Some(format!("\x0313{}", err));
                        is_over_limit = err.is_over_limit();
                    }
                }
                main_thread(move |hc| -> Result<(), ContextError> {
                    if let Some(ctx) = hc.find_context(&network, &channel) {
                        if !mode_char.is_empty() {
                            ctx.emit_print(
                                msg_type, &[&sender, &msg, &mode_char, "~"])?;
                        } else {
                            ctx.emit_print(msg_type, &[&sender, &msg, "~"])?;
                        }
                        ctx.print(&format!("\x0311{}", message))?;
                        if let Some(emsg) = &emsg { 
                            ctx.print(emsg)?;
                            
                            if is_over_limit {
                                ctx.command("OFFLANG")?;
                            }
                        }
                    } else {
                        hc.print("Failed to get context.");
                    }
                    Ok(())
                });
            });
            Some(())
        }}().is_none() {
            // If we get here, either `strip()` or `get_info()` returned None.
            hc.print("\x0313\
                     Translator Error: Basic failure retrieving channel \
                     information, or unable to strip original message.");
        }
        Eat::Hexchat
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
///   the translation failed. The error will contain an aggregate of 
///   descriptions for each problem encountered during translation.
///
fn google_translate_free(text   : &str, 
                         source : &str, 
                         target : &str
                        ) -> Result<String, TranslationError> 
{
    // Optimizing the regex and agent using lazy_static wouldn't noticeably
    // improve performance for the user. Plus, static resources are very hard to
    // thoroughly clean up for when the plugin is being unloaded/reloaded.
    let expr  = Regex::new(r".+?(?:[.?!;]+\s+|$)").unwrap();
    let agent = ureq::AgentBuilder::new()
                      .timeout_read(
                           Duration::from_secs(TRANSLATION_SERVER_TIMEOUT)
                      ).build(); 
                     
    let mut translated = String::new();
    let mut errors     = vec![];
    let mut over_limit = false;

    // The translation service won't translate past certain punctuation, so we
    // break the message up into parts terminated by such punctuation and
    // treat each one as a separate translation while piecing the results 
    // together.
    for m in expr.find_iter(text) {
        let sentence = m.as_str();

        match translate_single(sentence, &agent, source, target) {
            Ok(trans) => {
                translated.push_str(&trans);
            },
            Err(err)  => {
                use SingleTranslationError as STE;

                let emsg = match err {
                    STE::StaticError(s) => {
                        s.to_string()
                    },
                    STE::DynamicError(s) => {
                        s
                    },
                    STE::OverLimit(s) => {
                        over_limit = true;
                        s.to_string()
                    }
                };
                errors.push(emsg);
                translated.push_str(sentence);
            },
        }
    }
    if !errors.is_empty() {
        // Error will contain the partially translated text, deduplicated
        // error messages, and indicate if the translation limit was reached.
        errors.sort_unstable();
        errors.dedup();
        Err( TranslationError::new(translated, errors.join(" "), over_limit) )
        
    } else {
        // Each sentence translated went successfully.
        Ok( translated )
    }
}

/// Represents errors encountered when doing a single translation. This
/// error is generated by `translate_single()`.
/// # Variants
/// * `StaticError`  - A predicted error with a static error message.
/// * `DynamicError` - A freeform text error for unexpected errors.
/// * `OverLimit`    - Indicates that the translation server sent a response
///                    saying the user has used up all their translations
///                    in some amount of time.
///
#[derive(Debug)]
enum SingleTranslationError {
    StaticError  (&'static str),
    DynamicError (String),
    OverLimit    (&'static str),
}

/// Translates a single phrase, or sentence - one without multiple clauses 
/// separated by stop punctuation like a period.
/// # Arguments
/// * `sentence`    - The phrase to translate.
/// * `agent`       - The network agent that will send the HTTPS GET.
/// * `source`      - The source language to translate from.
/// * `target`      - The target language to translate to.
/// # Returns
/// * A `Result` with either a `String` if the translation was successful; or
///   a `SingleTranslationError` if not.
///
fn translate_single(sentence : &str, 
                    agent    : &ureq::Agent,
                    source   : &str,
                    target   : &str
                   ) -> Result<String, SingleTranslationError>
{
    use SingleTranslationError::*;

    let url = format!("https://translate.googleapis.com/\
                       translate_a/single\
                       ?client=gtx\
                       &sl={source_lang}\
                       &tl={target_lang}\
                       &dt=t&q={source_text}",
                      source_lang = source,
                      target_lang = target,
                      source_text = urlparse::quote(sentence, b"")
                                    .expect("URL message escaping failed."));
                                    
    let tr_rsp = agent.get(&url).call()
                 .map_err(|_| StaticError("Failed to get response from \
                                           translation server."))?;
    if tr_rsp.status_text() == "OK" {
    
        let rsp_txt = tr_rsp.into_string()
                      .expect("Failed to get text for HTTP response body.");
                            
        let tr_json = serde_json::from_str::<Value>(&rsp_txt)
                      .map_err(|_| StaticError("Received invalid response \
                                                format from server."))?;
        let trans   = tr_json[0][0][0].as_str()
                      .ok_or( StaticError("Received invalid response \
                                           format from server.") )?;
        let mut trans = trans.to_string();
        if sentence.ends_with(' ') {
            trans.push(' ');
        }
        Ok(trans)
        
    } else if tr_rsp.status() == 403 {
        Err( OverLimit("Server translation limit reached.") )
    } else {
        Err( DynamicError(tr_rsp.status_text().to_string()) )
    }
}

/// Implements the /LISTLANG command - prints out a list of all languages 
/// that the translation web services support.
///
#[allow(clippy::many_single_char_names)]     
fn on_cmd_listlang(hc        : &Hexchat, 
                   word      : &[String], 
                   _word_eol : &[String], 
                   _userdata : &mut UserData
                  ) -> Eat 
{
    if word.len() == 1 {
        hc.print("");
        hc.print("\x0311\
                  ------------------------ Supported Languages \
                  ------------------------");
        let langs = &SUPPORTED_LANGUAGES;
        
        for i in (0..langs.len()).step_by(3) {
            let (a, b) = langs[i];
            let (c, d) = langs[i + 1];
            let (e, f) = langs[i + 2];
            hc.print(
                &format!("\x0311{:-15}{:3}        {:-15}{:3}        {:-15}{:3}", 
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

/// Translation error. The error object will contain either a mix of translated
/// and untranslated messages - if some succeeded and some didn't. Or, just
/// untranslated text accessible from `get_partial_trans()`. The display
/// of the error will be an accumulated set of each unique error that occurred
/// during the translation. If the server indicated the user is over their
/// translation limit, `is_over-limit()` will reflect that.
///
#[derive(Debug)]
struct TranslationError {
    partial_trans : String,
    error_msg     : String,
    over_limit    : bool,
}

impl TranslationError {
    /// Constructs the translation error.
    /// # Arguments
    /// * `partial_trans`   - Translated and untranslated portions of the 
    ///                       original text.
    /// * `error_msg`       - The aggregate of error messages that occurred
    ///                       during the translation.
    /// * `over_limit`      - A bool indicating whether the server responded
    ///                       with a 403 error.
    ///
    fn new(partial_trans: String, error_msg: String, over_limit: bool) -> Self {
        TranslationError { partial_trans, error_msg, over_limit }
    }
    
    /// Returns the parts of translated and untranslated text - in the same
    /// order as the original text.
    ///
    fn get_partial_trans(&self) -> &str {
        &self.partial_trans
    }
    
    /// Indicates whether the translator server responded with a 403 error
    /// which means the number of translations per given span of time has 
    /// been exceeded.
    ///
    fn is_over_limit(&self) -> bool {
        self.over_limit
    }
}

impl Error for TranslationError {
    /*
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let Some(err) = &self.source_error {
            Some(err.as_ref())
        } else { None }
    }
    */
}

impl fmt::Display for TranslationError {

    /// Displays the aggregate of error messages that occurred during the 
    /// translation.
    ///
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Translation Error: {}", self.error_msg)
    }
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
    
    
    
