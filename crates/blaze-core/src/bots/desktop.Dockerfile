# BlBots private desktop: XFCE + Firefox on a virtual screen, viewable with
# noVNC on port 6080 (published to 127.0.0.1 only by Blaze). Built locally by
# Blaze IDE / blaze the first time a bot uses its computer.
FROM debian:bookworm-slim
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb x11vnc novnc websockify xfce4 xfce4-terminal dbus-x11 \
      xdotool scrot firefox-esr fonts-dejavu ca-certificates curl git python3 procps \
  && rm -rf /var/lib/apt/lists/* \
  && useradd -m -s /bin/bash bot \
  && mkdir -p /home/bot/work && chown -R bot:bot /home/bot
RUN printf '%s\n' '#!/bin/sh' \
      'Xvfb :1 -screen 0 1280x800x24 -nolisten tcp &' \
      'sleep 1' \
      'su bot -c "DISPLAY=:1 dbus-launch startxfce4" >/dev/null 2>&1 &' \
      'x11vnc -display :1 -forever -shared -nopw -localhost -rfbport 5900 -quiet &' \
      'exec websockify --web /usr/share/novnc 6080 localhost:5900' \
      > /usr/local/bin/start-desktop && chmod +x /usr/local/bin/start-desktop
EXPOSE 6080
CMD ["/usr/local/bin/start-desktop"]
