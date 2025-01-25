const { NotificationAck } = require("./generated/notification_service_pb.js");
const {
  NotificationClient,
} = require("./generated/notification_service_grpc_web_pb.js");

// ✅ Use correct gRPC-Web endpoint with `https://`
const notificationService = new NotificationClient(
  "https://beta.beckn.uat.juspay.net:50051",
  null,
  {
    withCredentials: true, // Ensure no CORS credential issues
  }
);

// ✅ Create the metadata correctly
const metadata = {
  "token-origin": "DriverDashboard",
  token: "618da5ec-c349-4715-8537-f5ca0bba8a5f",
};

// ✅ Prepare the initial request
const request = new NotificationAck();
request.setId("0-0");

const startStream = () => {
  // ✅ Start gRPC-Web streaming correctly
  const stream = notificationService.streamPayload(request, metadata);

  stream.on("data", (response) => {
    console.log("📩 Received Notification:", response.toObject());
  });

  stream.on("status", (status) => {
    console.log("🔄 Stream Status:", status);
  });

  // ✅ Handle errors properly
  stream.on("error", (err) => {
    console.error("❌ Stream Error:", err);
  });

  // ✅ Handle when stream ends
  stream.on("end", () => {
    console.log("🔄 Stream Ended. Reconnecting...");
    setTimeout(() => {
      console.log("♻️ Reconnecting...");
      startStream(); // Automatically restart the stream
    }, 5000);
  });
};

startStream();
