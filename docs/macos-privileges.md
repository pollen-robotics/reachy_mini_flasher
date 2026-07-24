# macOS privilege elevation & disk access

Writing an OS image to the CM4 eMMC means **writing to a raw block device**
(`/dev/rdiskN`). On macOS this needs two distinct things, which are often
confused:

1. **Root / admin authorization** to run the write.
2. **Full Disk Access (TCC)** for the process that actually opens the device,
   *even when it is already root*.

Both are covered below.

---

## 1. Getting root (the password prompt)

There is no built-in privilege elevation in Tauri. The community options:

| Approach | Shows app name in prompt? | Signing required? | Notes |
|---|---|---|---|
| **A. `osascript ... with administrator privileges`** (current) | No - shows `osascript` (a custom `with prompt` message can be added) | No | Simplest. What we ship today. |
| **B. `sudo --askpass` + custom dialog** (balenaEtcher) | No - custom dialog, but not the app name | No | Full control of the dialog, but the app collects the password itself; criticized on security grounds. |
| **C. Signed privileged helper via `SMAppService` + XPC** | **Yes** - app name + icon, asked once | **Yes** (Developer ID + notarization) | The Apple-sanctioned way. Heavy but correct. |

Notes:
- `AuthorizationExecuteWithPrivileges` (the old API that showed the app name) is
  **deprecated and broken** on recent Xcode SDKs. Do not use.
- As long as elevation goes through `osascript`, the requesting process shown in
  the dialog is `osascript`, regardless of the app's signature - because
  `osascript` is the binary triggering the authorization.

### What we do now

- **Flash (disk write)** uses **`authopen`** (Option D below), not osascript. This
  both gets root *and* dodges the Full Disk Access problem in one step.
- **rpiboot preparation** (`rpiboot.rs`) still uses osascript
  (`do shell script ... with administrator privileges`) because it needs root
  to talk to the CM4 over USB, which is not a disk-access (TCC) problem.

  Two macOS gotchas bit us here, both only in a **dev checkout under
  `~/Documents`** (a TCC-protected folder). The elevated shell is a *separate*
  TCC subject from the app, so:
  1. Its startup `getcwd()` fails if it inherits a cwd under the protected
     folder -> `shell-init: ... getcwd: ... Operation not permitted (255)`,
     killing the script before rpiboot runs. Fixed by launching `osascript`
     with `current_dir("/")`.
  2. Even with a valid cwd, the root process **cannot read** rpiboot's boot
     files if they live under the protected folder -> rpiboot reports
     `No 'bootcode' files found` and prints its help. Fixed by staging the
     binary + `mass-storage-gadget64` dir into the app cache
     (`~/Library/Caches/...`, not TCC-protected) in the app's own user context
     first (`stage_artifacts_macos`), then running rpiboot from there.

  Neither hits a packaged app, whose resources live in the `.app` bundle.

### Option D: `authopen` (the one we ship for flashing)

`/usr/libexec/authopen` is an Apple-signed setuid-root helper that opens a file
with an authorization prompt and passes the descriptor back to us over a socket
(`-stdoutpipe`, SCM_RIGHTS). Because the privileged `open()` happens inside an
Apple binary, **no Full Disk Access is needed** and we never touch osascript for
the write:

```
authopen -stdoutpipe -w /dev/rdiskN   # prompts admin, returns an fd
```

`src-tauri/src/flash.rs` (`open_via_authopen`) pairs a UNIX socket, hands one end
to authopen as stdout, receives the fd with the `sendfd` crate, and writes the
image to it in-process. Downside: the dialog says "authopen", not the app name
(same branding caveat as osascript). Upside: works unsigned, no manual settings.

### Real projects, for reference

- **balenaEtcher** (`lib/shared/sudo/darwin.ts`): `sudo -E --askpass sh -c ...`
  with `SUDO_ASKPASS` pointing to a bundled osascript dialog. Acknowledged by
  maintainers as suboptimal (issues #3321, #4052) - the "correct" target is a
  signed helper + XPC.
- **Raspberry Pi Imager / most pro Mac apps**: signed privileged helper via
  `SMJobBless` (now `SMAppService`) + XPC.

### Recommendation

- **Now (unsigned, dev/internal):** Option D (`authopen`) for the flash write -
  root + no Full Disk Access, no manual settings. Already implemented.
- **At packaging (Developer ID + notarization):** optionally move to Option C
  (`SMAppService` helper + XPC) to get "Reachy Mini Flasher" as the requester
  name and a single up-front authorization. Option D remains a perfectly fine
  fallback.

---

## 2. Full Disk Access (the real blocker after the password)

Symptom, **after** entering the password:

```
failed to open target '/dev/rdisk6': Operation not permitted (os error 1)
```

This is **not** a mount issue (that would be "Resource busy"/EBUSY) and **not** a
real permission bug. On macOS, opening a raw disk device requires the
**responsible process** to have **Full Disk Access**, *even as root*. This is the
classic `sudo dd of=/dev/rdiskN -> Operation not permitted` case.

Granting Full Disk Access to the dev binary does **not** fix it: when the write
runs as a root child of `osascript`/`security_authtrampoline`, TCC does not
attribute the access to that binary, so it stays blocked.

### How we solved it: `authopen` (no Full Disk Access at all)

Instead of opening the device ourselves (as root, blocked by TCC), we ask the
Apple-signed setuid helper `/usr/libexec/authopen` to open it and pass us the
descriptor (see Option D above). The privileged `open()` happens inside Apple's
binary, which is not subject to the app's Full Disk Access state, so the write
just works after the admin prompt - in dev **and** in a packaged app, signed or
not. Users never have to touch System Settings.

### Why Option C avoids this

A privileged helper installed as a launchd daemon (via `SMAppService`) runs in a
system context that can access disk devices, so the end user does not have to
toggle Full Disk Access manually. This is another reason to move to Option C at
packaging time.

---

## TL;DR

- Admin prompt = normal (root needed to write a disk).
- Flash write goes through `authopen`, so **no Full Disk Access** is ever
  required - the `Operation not permitted` dead end is gone.
- rpiboot prep still uses osascript (USB, not disk - unaffected by TCC).
- Branding caveat: the prompt says "authopen". A signed `SMAppService` helper
  (Option C) would show the app name and ask once - a packaging-time nicety.
