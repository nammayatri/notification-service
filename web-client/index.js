const { NotificationAck } = require("./generated/notification_service_pb.js");
const {
  NotificationClient,
} = require("./generated/notification_service_grpc_web_pb.js");

// ✅ Use correct gRPC-Web endpoint with `https://`
const notificationService = new NotificationClient(
  "https://grpc.sandbox.moving.tech:50051",
  null,
  {
    withCredentials: true, // Ensure no CORS credential issues
  }
);

// ✅ Create the metadata correctly
const metadata = {
  "token-origin": "DriverDashboard",
  token: "d8a51bfb-0b17-433c-a6db-6bb2c045d599",
};

// ✅ Prepare the initial request
const request = new NotificationAck();
request.setId("0-0");

const startStream = () => {
  // ✅ Start gRPC-Web streaming correctly
  const stream = notificationService.serverStreamPayload(request, metadata);

  stream.on("data", (response) => {
    console.log("📩 Received Notification:", response.toObject());
  });

  stream.on("status", (status) => {
    console.log("🔄 Stream Status:", status);
  });

  // ✅ Handle errors properly
  stream.on("error", (err) => {
    console.error("❌ Stream Error:", err);
    setTimeout(() => {
      console.log("♻️ Reconnecting...");
      startStream(); // Automatically restart the stream
    }, 1000);
  });

  // ✅ Handle when stream ends
  stream.on("end", () => {
    console.log("🔄 Stream Ended. Reconnecting...");
    setTimeout(() => {
      console.log("♻️ Reconnecting...");
      startStream(); // Automatically restart the stream
    }, 1000);
  });
};

startStream();
