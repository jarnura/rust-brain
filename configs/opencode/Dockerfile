FROM node:22-slim
RUN apt-get update && apt-get install -y curl git ripgrep fd-find && rm -rf /var/lib/apt/lists/*
# fd-find installs as fdfind — create fd alias
RUN ln -sf /usr/bin/fdfind /usr/local/bin/fd
RUN npm install -g opencode-ai
RUN useradd -m -s /bin/bash opencode
# Create directories with correct permissions for the opencode user
RUN mkdir -p /home/opencode/.local/share/opencode/bin /home/opencode/.local/state /home/opencode/.config/opencode /workspace/target-repo && \
    chown -R opencode:opencode /home/opencode /workspace
USER opencode
# Set project directory to /workspace/target-repo so opencode's sandbox
# allows file access there. The target repo is mounted read-only via
# docker-compose volume.
WORKDIR /workspace/target-repo
EXPOSE 4096
CMD ["opencode", "serve", "--port", "4096", "--hostname", "0.0.0.0"]
