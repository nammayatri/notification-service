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

const connect = () => {
  // Create a new gRPC client
  client = new notificationProto.Notification(
    "beta.beckn.uat.juspay.net:50051",
    grpc.credentials.createSsl()
  );

  const metadata = new grpc.Metadata();
  metadata.add("token-origin", "DriverDashboard");
  metadata.add("token", "618da5ec-c349-4715-8537-f5ca0bba8a5f");

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

    // Ensure previous client is closed
    console.log("Closing previous client connection...");
    client.close();

    // Ensure previous stream is properly closed before reconnecting
    console.log("Closing previous stream before reconnecting...");
    stream.cancel();

    // Reconnect
    connect();
  });

  stream.on("error", (err) => {
    console.error("Stream error:", err);

    if (
      err.code === grpc.status.UNAVAILABLE ||
      err.code === grpc.status.INTERNAL
    ) {
      console.log("Stream error. Terminating...");

      // Ensure previous client is closed
      console.log("Closing previous client connection...");
      client.close();

      // Ensure previous stream is properly closed before reconnecting
      console.log("Closing previous stream before reconnecting...");
      stream.cancel();
      stream.end();
    } else {
      console.log("Unexpected error.", err);
    }
  });
};

// Start the connection
connect();
