const grpc = require("@grpc/grpc-js");
const protoLoader = require("@grpc/proto-loader");

// Path to your `.proto` file
const PROTO_PATH =
  "../crates/notification_service/protos/notification_service.proto";

// Load `.proto` file dynamically
const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
  keepCase: true,
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true,
});

// Load gRPC package
const notificationProto =
  grpc.loadPackageDefinition(packageDefinition).notification_service;

let client = null;
let stream = null;

const connect = () => {
  // Ensure previous stream is properly closed before reconnecting
  if (stream) {
    console.log("Closing previous stream before reconnecting...");
    stream.cancel();
  }

  // Ensure previous client is closed
  if (client) {
    console.log("Closing previous client connection...");
    client.close();
  }

  // Create a new gRPC client
  client = new notificationProto.Notification(
    "beta.beckn.uat.juspay.net:50051",
    grpc.credentials.createSsl()
  );

  const metadata = new grpc.Metadata();
  metadata.add("token-origin", "DriverDashboard");
  metadata.add("token", "f62718f5-e2ac-4334-9b98-65b4438c8890");

  console.log("Establishing new gRPC connection...");
  stream = client.StreamPayload(metadata);

  // Handle incoming notifications
  stream.on("data", (response) => {
    console.log("Received notification:", response);

    // Send acknowledgment
    stream.write({ id: response.id });
  });

  stream.on("end", () => {
    console.log("Stream ended. Reconnecting...");
    connect();
  });

  stream.on("error", (err) => {
    console.error("Stream error:", err);

    if (
      err.code === grpc.status.UNAVAILABLE ||
      err.code === grpc.status.INTERNAL
    ) {
      console.log("Stream error. Reconnecting...");
      connect();
    } else {
      console.log("Unexpected error.", err);
    }
  });
};

// Start the connection
connect();
