# Browser SSH

Apache Guacamole 1.6.0 uses its official JSON authentication extension. The dashboard exchanges a signed, short-lived connection definition on the server; SSH private keys never reach the browser. Browser session tokens are sensitive. The outer reverse proxy authenticates every Guacamole request, validates origins, and proxies WebSockets.

Supply a random 16-byte hex JSON_SECRET_KEY in private `.env`, and the same value in dashboard `private/guacamole-key`. Supply verified SSH host keys in `ssh_known_hosts` (mounted read-only into guacd) and `private/guacamole_known_hosts`. Never use unauthenticated ssh-keyscan output as identity verification. Each target needs tmux and a dedicated authorized SSH key; disable forwarding and restrict its source to the dashboard host.

Run `docker compose up -d` in this directory. The web service binds only loopback port 8095; guacd is not published. The provided nginx example must be adapted to your exact hostname and private password file. TLS termination is required outside it. Existing authenticated Caddy deployments may proxy these same loopback endpoints directly.

The shell persists in a separate tmux socket. Closing the browser does not end its shell. Explicitly exit sensitive sessions. Do not claim physical phone keyboard qualification from desktop device emulation.

Sources: https://guacamole.apache.org/doc/gug/guacamole-docker.html and https://guacamole.apache.org/doc/gug/json-auth.html
