
# Hexchat Translator

This is a plugin to Hexchat that provides translated chat features enabling one 
to easily chat with people in other tongues. 

## Hexchat Commands
* `/LISTLANG` 
    * Lists all the supported langauges.
* `/SETLANG <your-language> <other-langauge>`
    * Sets the the languages to translate to/from in the current channel.
* `/LSAY <message>`
    * Like `/SAY`, sends a translated message to the IRC chat channel.
* `/LME <emote-message>`
    * Like `/ME`, sends a translated emote message to the channel.
* `/OFFLANG`
    * Turns off translation in the current channel.

The help for these 
can be accessed through the Hexchat "/HELP" command.

## Binaries
* Linux   - libhexchat_translator.so
* Windows - hexchat_translator.dll
* Mac     - libhexchat_translator.dylib 

You can download these binaries individually from the
[release](https://github.com/ttappr/hexchat_translator/releases/tag/ver-0.1.1),
or get the whole package. 

This plugin is stable, but experimental. It interact's with Google's free 
translation web service which generously limits the number of translations per 
hour. 

To add it to Hexchat, you can put the relevant binary in the "addons" 
folder of your system's Hexchat config directory.
* `~/.config/hexchat/` for Linux
* `%APPDATA%\HexChat` on Windows

Or you can load it directly from the UI: 
* `Window > Plugins and Scripts > Load` - then navigate to the file and load it.

## Rust Hexchat API
This project uses a [Rust Hexchat API lib](https://github.com/ttappr/hexchat_api), 
which other developers may find useful for writing their own Rust Hexchat 
plugins. It has some nice features like
* A thread-safe API.
* Simple user_data objects.
* Abstractions like `Context` that make it simple to interact with specific tabs/windows in the UI.
* Panic's are caught and displayed in the active window.

