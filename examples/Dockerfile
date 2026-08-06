FROM alpine:3.20
RUN addgroup -S runner && adduser -S -G runner runner
WORKDIR /workspace
USER runner
ENTRYPOINT ["/bin/sh"]
