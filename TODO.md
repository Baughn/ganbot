# TODO

## Implemented
- [x] Discord `prompt` slash command now flows through the action broker, persists progress metadata, and posts a fresh ping on completion.

## Planned Enhancements
- [ ] Surface broker `Progress` updates to Discord with richer status text/ETAs.
- [ ] Add Discord gallery controls (buttons `U1…Un`) once prompt results expose selectable images.

## Open Tasks
- [ ] Finish broker integration for Discord `dream`/`select` commands (submit via broker, persist state, deliver completions).
- [ ] Route the IRC `edit` command through the broker so edits survive restarts (`src/network/irc.rs:846`).
- [ ] Fix IRC authentication.
