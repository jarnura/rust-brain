FROM node:22-slim
RUN apt-get update && apt-get install -y curl git ripgrep fd-find rsync && rm -rf /var/lib/apt/lists/*
RUN ln -sf /usr/bin/fdfind /usr/local/bin/fd
RUN npm install -g opencode-ai @ai-sdk/openai-compatible
RUN useradd -m -s /bin/bash opencode
COPY docker-entrypoint.sh /usr/local/bin/opencode-entrypoint.sh
RUN chmod +x /usr/local/bin/opencode-entrypoint.sh
RUN mkdir -p /home/opencode/.local/share/opencode/bin /home/opencode/.local/state /home/opencode/.config/opencode /workspace/target-repo /workspace/target-repo-work && \
    chown -R opencode:opencode /home/opencode /workspace
ENV NODE_PATH=/usr/local/lib/node_modules
USER opencode
WORKDIR /workspace/target-repo-work
EXPOSE 4096
ENTRYPOINT ["/usr/local/bin/opencode-entrypoint.sh"]
