FROM node:22-bookworm-slim
WORKDIR /app/native-stores
COPY native-stores/package*.json ./
RUN npm ci --omit=dev --ignore-scripts && npm cache clean --force
COPY native-stores/src ./src
COPY proto /app/proto
USER node
EXPOSE 50055
CMD ["node", "src/main.mjs"]
