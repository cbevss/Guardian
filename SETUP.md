# Guardian setup guide

Guardian lets parents see and control a child's PC, and decide which apps and
websites he can use and when.

## Installing

Download both installers from the latest GitHub Actions run (**Actions** tab >
newest run > **Guardian-installers**) or from a release.

### 1. Your PC (and your mum's): `Guardian-Parent-PC-Setup.exe`
- Installs the viewer and the home relay, which starts quietly when you sign in.
- Puts a **Guardian** shortcut on the desktop.
- Allows Guardian through Windows Firewall on **private** networks. Make sure your
  home Wi-Fi is set to *Private* in Windows network settings.

### 2. His PC: `Guardian-Child-PC-Setup.exe`
1. Give his Windows account **standard** permissions (Settings > Accounts >
   Other users > Change account type > Standard User). Keep the admin
   password between you and your mum.
2. Sign in with an **admin** account and run the installer.
3. At the end it shows a **Device ID** and **pairing code**. Write them down.

He can't close Guardian (Task Manager says *Access denied*) or uninstall it,
because both need the admin password.

### 3. Connect
Open **Guardian** on your PC, enter the Device ID and pairing code, tick
**Remember on this device**, then press **Connect**.

## Using it

| Button | What it does |
|---|---|
| **Take control** | Use his mouse and keyboard. While on, all his restrictions are paused for you. They come back the moment you stop or the connection drops. |
| **Rules** | Approved/Denied apps and websites, and screen-time hours. |
| **Send my clipboard** | Copies your clipboard to his PC. (His copies come to you automatically.) |

### Approval popups
If he opens an app or website that isn't Approved or Denied, it's blocked and
both of you get a popup:

- **Temporary Access**: apps, until he closes it; websites, for 1 hour
- **Approve**: always allowed
- **Deny**: always blocked

Whoever answers first decides. To change your mind later, add it to the other
list in **Rules**; it's moved automatically.

**Apps** are approved as a whole folder, so an app's helper programs keep
working. Games inside a launcher (Steam, Epic, Riot...) each get their own
popup, so approving Steam doesn't approve every game.

**Websites** are approved as a whole site (youtube.com covers www and m.youtube.com).
In the minute after you approve one, the extra servers it loads (images, video)
are added to it automatically so the site works properly.

Expect a burst of popups on the first day (Edge, Settings, and so on).
Approve the everyday ones once and it quietens down.

### Screen time
In **Rules > Screen time**, tick the box and give times per day, like
`16:00-20:00`. Outside those times the PC locks. A day left empty stays locked
all day. Taking control pauses this for you.

## Connection modes

| Mode | Relay runs on | Cost | Works away from home? |
|---|---|---|---|
| **Home** (default) | Your PC | Free | No |
| **Relay** | A cloud server | ~£4/month | Yes |

To switch later: on the server run `guardian-relay` with `GUARDIAN_MODE=relay`
behind HTTPS (e.g. Caddy). Then, as admin on his PC, edit
`C:\ProgramData\Guardian\agent.json`: set `"mode": "relay"` and
`"relay": "wss://your-server"`, and restart the *Guardian* service.

## Extra safety net
Allowed website lookups go through **Cloudflare for Families** (1.1.1.3), which
also blocks adult and malware sites. It can be changed with `dns_upstream` in
`agent.json`.

## Known limits
- Browser **VPN/proxy extensions** can tunnel around the website filter. Deny
  VPN apps when they're requested, and keep an eye on browser extensions.
- Requests and live view need a parent's PC on and the Guardian page open (Home mode).
- No mouse/keyboard control while Windows shows its lock screen or an admin (UAC) prompt.

## Uninstalling
- His PC: sign in as admin, then Settings > Apps > Guardian > Uninstall.
  This puts his DNS and browser settings back to normal.
- Your PC: Settings > Apps > Guardian Parent > Uninstall.

## For developers
```
cargo test --workspace
cargo run -p guardian-relay          # viewer at http://localhost:8080
cargo run -p guardian-agent          # test mode: no service, no system changes
```
