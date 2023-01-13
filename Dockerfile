FROM debian:stable-20230109-slim
RUN apt-get update && apt-get install libmariadb-dev ca-certificates -y && apt-get clean
COPY target/release/ultrafinance /ultrafinance
COPY frontend/dist /frontend/dist
EXPOSE 3000
CMD ["/ultrafinance", "server", "start"]
