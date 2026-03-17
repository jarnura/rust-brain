FROM node:22-slim
RUN npm install -g opencode-ai
RUN useradd -m -s /bin/bash opencode
USER opencode
WORKDIR /home/opencode
EXPOSE 4096
CMD ["opencode", "serve", "--port", "4096", "--hostname", "0.0.0.0"]
