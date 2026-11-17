# Sideglow

[한국어](README.md) | [日本語](README.ja.md) | **English**

**A Discord chat and translation overlay for games**

Sideglow is a Windows chat window that displays conversations from a Discord channel over your game and lets you send messages using a keyboard shortcut. Optional AI translation lets you read the original messages and their translations together.

Built with Rust and a native Windows UI, Sideglow focuses on a small window and essential features. It works alongside the Discord desktop app. It does not read a game's own chat or translate the screen using OCR.

> **[Sideglow](https://github.com/PN-7/sideglow/releases)**

## Before you start

**Install the Discord desktop client, sign in with the account you want to use, and leave it running.** Sideglow receives channel messages through this desktop app. Running only the browser or mobile version of Discord is not sufficient.

| Requirement | What you need |
|---|---|
| Operating system | A Windows 10/11 64-bit PC with an internet connection |
| Discord desktop app | Installed, signed in, and running. [Download Discord](https://discord.com/download) |
| Discord account | Access to the server and text channel you want to use. Use the same account for browser authorization and the desktop app. |
| Connection information | A connection code from the organizer or a friend, or a Discord application ID and channel link |
| Sending messages | A webhook URL for the same channel is also required. Without a webhook, you can receive messages only. |
| Game display mode | Designed for windowed or borderless windowed mode. |
| AI translation — optional | Only the person running translation needs a key for the selected API provider. OpenAI, OpenAI-compatible services, Anthropic, and Gemini are supported. People who only receive translations do not need a key. |

**The current connection method is subject to Discord's app permission restrictions.** The RPC permissions needed to receive chat depend on the app's approval status. Creating a Discord app or accepting a tester invitation does not guarantee that every account can connect. If you are setting this up for the first time, verify that both your account and a friend's account can actually receive messages before distributing the app to friends. See the [Discord connection guide](docs/SETUP.md) for the procedure and [Discord's official OAuth2 documentation](https://docs.discord.com/developers/topics/oauth2) for the permission scopes.

## Features

- **Chat over your game:** View original Discord channel messages and translated replies together. Authors are shown by nickname without the webhook prefix, and message times use the local time configured in Windows.
- **Type with a shortcut:** Press `Enter` by default to open the input box in your selected game. You can change the shortcut in Settings.
- **Click through and adjust the window:** Mouse clicks normally pass through to the game. Select Sideglow with `Alt+Tab` to move the window or drag its edges to resize it.
- **Channel tabs:** Save up to 8 channels. Only the selected tab receives and translates messages in real time.
- **Readable appearance settings:** Adjust the system font, text size, message/author/translation colors, character and line spacing, text outlines, background color, and opacity. Preview appearance changes before saving.
- **Hide older conversations:** Hide conversations from the overlay as they reach the configured age. The actual messages remain in the Discord channel.
- **Optional AI translation:** Choose OpenAI, an OpenAI-compatible service, Anthropic, or Gemini to translate new channel messages using one person's API key. Everyone receives the results, and translation can be turned on or off in the management view.
- **Personal romanized Japanese option:** People who type Japanese using the alphabet can enable **Detect romanized Japanese in my messages** in Settings and save. It is off by default, and people who are not running translation can also select it.
- **Interface languages:** Korean, Japanese, and English are available. UI text can be edited in external JSON files.

## Download and run

**The source is currently available in this repository, but an executable release has not yet been published on GitHub Releases.** GitHub's **Code → Download ZIP** downloads the source archive. To build the executable yourself, follow [Building from source](#building-from-source) below.

If you have received a distribution ZIP containing the Windows x64 executable:

1. Obtain the Sideglow distribution ZIP for Windows x64.
2. Extract the ZIP and keep the executable and the `locales` folder in the same folder.
3. Sign in to the Discord desktop app, then open the executable, `sideglow.exe`.
4. Follow the connection steps below. When updating, fully exit the existing program before starting the new version.

You do not need to install Rust or Node.js separately to use the executable. `start-demo.cmd` is for viewing a sample screen; use the regular executable for an actual Discord connection.

## If a friend gave you connection information

1. Open **Settings → First connection / sign in… → Connect with a code or links from a friend**.
2. Apply the code through **Paste connection code… → Paste code → Fill settings**. You can also enter the app ID, channel link, and webhook URL directly. Downloading a JSON file is optional.
3. Click **Sign in to Discord in browser** and authorize with your own account. Make sure the Discord desktop app and browser are using the same account.
4. Check the reception status and readiness to send messages, then click **Finish**. If you have not entered a webhook, you can receive messages only.
5. Start your game, select it under **Running apps and shortcut** in Settings, and click **Save**. If it is not listed, click **Refresh list**.
6. Select Sideglow with `Alt+Tab`, make sure the chat toggle at the top is **ON**, then return to your game. Press `Enter` by default to start typing a message.

**Each friend does not need to create a new app or bot in the Discord Developer Portal.** Some apps may require the organizer to send an App Testers invitation and the friend to accept it. Connection codes do not contain sign-in information, so everyone must authorize with their own Discord account.

## If you are setting up the connection for the first time

One organizer prepares the Discord application and channel. Follow the steps in **Settings → First connection / sign in… → Set up for the first time** in Sideglow.

1. Prepare a Discord application and configure its app ID and sign-in redirect URL.
2. **First verify that you can actually receive chat messages** from the channel you want to use.
3. Once reception works, prepare a sending webhook for the same channel.
4. Share the connection with friends through **Settings → Connection code… → Copy current connection code**.

Register the sign-in redirect URL in your browser under **Discord Developer Portal → your app → OAuth2 → Redirects**.

```text
http://127.0.0.1:43821/callback
```

This URL lets the browser return to the program on this PC after sign-in authorization. It does not belong in Sideglow's channel link field. Follow the [step-by-step connection guide](docs/SETUP.md) for Public Client settings, app permissions, tester invitations, and error troubleshooting.

The default direct connection method does not require a separate relay server or hosting for a translation bot. Keep the Discord desktop app running while you use Sideglow.

## Basic controls

| Action | Result |
|---|---|
| `Enter` in the selected game | Start typing a chat message. The shortcut can be changed. |
| `Enter` while typing | Send the message. Close the input box if it is empty or contains only whitespace. |
| `Esc` while typing | Cancel sending and return to the game. Keep the draft. |
| Select Sideglow with `Alt+Tab` | Open the management view to move or resize the window, switch tabs, and use Settings. |
| Scrollbar on the right of the management view | Browse earlier conversations. |
| Mouse wheel / `Page Up` / `Page Down` while typing | Browse earlier conversations. |
| Chat `ON/OFF` at the top | Enable or disable the in-game display and input shortcut. |
| Translation `ON/OFF` at the top | Enable or disable shared AI translation for the current channel on this PC. |

Keyboard focus moves to the chat window while you type. After sending or closing the input box, focus returns to the game once you release `Enter` or `Esc`. If the shortcut conflicts with another action, choose a different key or key combination in Settings.

## Using AI translation

If you type Japanese using the alphabet, such as `arigatou`, select **Settings → Detect romanized Japanese in my messages → Save** in the main settings. Other users can leave it off. The choice applies only to messages you send from then on. Both the sender and the PC running translation must use **0.5.0-preview.7 or later**. Translation of Japanese written in Japanese characters and of Korean remains available regardless of this checkbox.

**Only one person per channel needs to set up and run translation.** That person's Sideglow and Discord desktop app must be running, with the channel tab to be translated selected.

1. The person running translation selects a provider under **Settings → AI translation settings… → API provider**. For an **OpenAI-compatible service**, enter the service's HTTPS base URL in **API base URL**. The official addresses for the other three providers are fixed.
2. Enter your key in **translation API key · protected for this Windows account**, then set **Model ID · editable** and Language A/B. Enter model IDs for other providers manually. OpenAI model presets and Fast are available only when OpenAI is selected.
3. Enable **Run shared translation on this PC**, click **OK** in the AI settings, then click **Save** in the main settings.
4. Other users leave shared translation off and enable **Show received AI translations**.
5. Send a new message and check that its translation appears below the original.

Each PC stores one translation provider, address, key, and model configuration shared by all channel tabs. **If you change the provider or API base URL, enter a key for the new connection.** If you change only the model for the same provider and address, leave the key field empty to keep the saved key. See [API provider settings](docs/API-PROVIDERS.md) for supported authentication methods and model requirements.

New text from regular users and messages from the input webhook registered in Settings are translated. Results are posted to the same Discord channel, where channel members can also see them. Past history and messages in inactive tabs are not translated in bulk.

API usage and charges apply to the provider account or project associated with the key entered by the person running translation. The program's license and external API fees are separate. Enabling translation on multiple PCs can cause duplicate requests and charges, so designate one person to run it.

The translation role is off by default. PCs with it disabled do not create translation workers or translation API communication sessions. Translation speed depends on the selected model, message length, network, and service conditions. Open **Settings → Last translation timing…** to check the queue, API, and Discord posting times.

See the [AI translation guide](docs/AI-TRANSLATION.md) for model selection and detailed settings.

## Channel and appearance settings

Add another channel using **+** in the `Alt+Tab` management view or **Settings → Channel tabs → Add…**. To change only the channel address or webhook, use **Edit channel / webhook…**. Of your saved tabs, only the currently selected one receives and translates messages in real time.

Keep the settings window open to preview fonts, colors, opacity, and spacing in the chat window. Enable **Separate translation style** to give original messages and translations different sizes and colors. **Save** keeps the changes; **Cancel** or **Close** restores the appearance from before the changes.

Choose the interface language under **UI language**. The interface language is separate from Language A/B for chat translation. See the [UI language files guide](docs/LOCALIZATION.md) for editing text and adding languages.

## Connection information and privacy

- Messages are sent **under the webhook's name** rather than your personal Discord account. If an existing translation bot ignores webhook messages, it may not translate these messages.
- **Webhook URLs and connection codes containing them grant permission to send messages.** Share them only with people you trust, and do not post them in public repositories, READMEs, or issues. A connection code is reversible encoding, not encryption.
- A connection code contains the app ID, channel, and optional webhook. It does not contain your Discord sign-in tokens or translation API key.
- When AI translation runs, the target message text, selected languages, and game terminology are sent to the selected API provider. If you select a compatible service, they are sent to the address you entered. Results are posted to the configured Discord channel.
- Saved API keys and sensitive connection information are protected using a mechanism tied to your Windows user account. You may need to enter them again or sign in again when moving to another PC or Windows account.

See the [connection code guide](docs/CONNECTION-CODE.md) for the scope of connection codes and how to handle them.

## Current scope of support

- Targets Windows 10/11 x64 and windowed or borderless windowed games.
- Uses a keyboard shortcut and a separate Windows window without injecting a DLL into the game process.
- Display, input, and focus restoration may be limited by exclusive fullscreen mode, games running as administrator, anti-cheat software, and input method environments.
- AI translation supports OpenAI Responses, OpenAI-compatible Chat Completions, Anthropic Messages, and Gemini GenerateContent. Compatible services must support Bearer authentication, JSON mode, and the other conditions described in [API provider settings](docs/API-PROVIDERS.md). Reading a game's own chat, OCR, speech recognition, and image translation are not supported.
- Third-party API integration was verified using mock tests with request and response fixtures. No actual third-party API keys were used to call those services during this work. Model access, response quality, and actual speed for each service have not been verified.
- This is a preview. Connection support for every game or Discord account, and consistent translation speed, are not guaranteed. The [validation record](docs/VALIDATION.md) distinguishes automated checks and Windows builds from verification in a real environment.

## Building from source

On Windows 10/11 x64, you need Rust's MSVC toolchain, Visual Studio C++ Build Tools, and the Windows SDK. See the [official Rust installation guide](https://www.rust-lang.org/tools/install).

Download the repository, then run the following in PowerShell from the project folder:

```powershell
rustup target add x86_64-pc-windows-msvc
.\scripts\build.ps1
```

The build creates `dist/sideglow.exe` and `dist/locales/`. Keep them together when running the app. Users who receive a compiled distribution do not need to install Rust or C++ build tools.

When redistributing the executable, also provide the source corresponding to that build, the license, and the notices for external components. The build script's `dist` output alone is not a complete distribution ZIP. See the [licensing guide](docs/LICENSING.md) for details.

## Documentation and issue reports

This README is translated into English; the detailed documents linked below remain in Korean.

| Document | Contents |
|---|---|
| [Discord connection guide](docs/SETUP.md) | First-time setup, connecting with friends, tester invitations, and authentication errors |
| [Connection code guide](docs/CONNECTION-CODE.md) | Sharing connection information without files |
| [AI translation guide](docs/AI-TRANSLATION.md) | Setting up the translation role, selecting a model, and checking costs and translation times |
| [API provider settings](docs/API-PROVIDERS.md) | Addresses, keys, and model requirements for OpenAI, compatible services, Anthropic, and Gemini |
| [Send timing guide](docs/SEND-LATENCY.md) | Checking delays when sending original messages |
| [UI language files guide](docs/LOCALIZATION.md) | Korean, Japanese, and English UI text, and editing additional languages |
| [Changelog](CHANGELOG.md) | Changes by version |
| [License scope](docs/LICENSING.md) | MPL scope, external components, and providing source |
| [Provenance review](docs/PROVENANCE.md) | Results of comparison with original sources and areas that remain unverified |

When reporting a problem, include the app version, Windows version, game display mode, steps to reproduce it, and the error message. For delays in sending or translating messages, attaching the latest timing record from Settings can help narrow down the cause. Hide API keys, webhook URLs, connection codes, and private conversations in screenshots.

## License

Sideglow's current project source is distributed under the [Mozilla Public License 2.0 (MPL-2.0)](LICENSE). Use, modification, and commercial distribution are permitted. When distributing a program with modified MPL code, you must provide recipients with a way to obtain the corresponding MPL source and your modifications.

**The Windows distribution ZIP includes both the executable and its corresponding editable project source.** The same archive contains `src/`, `Cargo.toml`, `Cargo.lock`, `build.rs`, `assets/`, `locales/`, build scripts, and notices.

External libraries and runtimes retain their respective original licenses. Keep the [third-party notices](THIRD_PARTY_NOTICES.md) and `third_party_licenses/`. The scope of the project's own code and external components in this release is described in [NOTICE](NOTICE) and the [licensing guide](docs/LICENSING.md). Confirmed evidence and limitations are recorded in the [provenance review](docs/PROVENANCE.md).

Discord and translation API terms and fees are governed by the policies of the respective services.

Sideglow is an independent project, not an official program of Discord, OpenAI, Anthropic, or Google.
