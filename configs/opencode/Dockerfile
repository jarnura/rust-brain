FROM node:22-slim
RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*
RUN npm install -g opencode-ai
RUN useradd -m -s /bin/bash opencode
# Create directories with correct permissions for the opencode user
RUN mkdir -p /home/opencode/.local/share/opencode/bin /home/opencode/.local/state /home/opencode/.config/opencode && \
    chown -R opencode:opencode /home/opencode
USER opencode
WORKDIR /home/opencode
EXPOSE 4096
CMD ["opencode", "serve", "--port", "4096", "--hostname", "0.0.0.0"]
