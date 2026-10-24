# The web app bundle (apps/web/dist), served at /app by the public Caddy.
# Built as a local-backend bundle: every service call goes to its own origin.
FROM caddy:2.10-alpine
COPY . /srv/frontend
