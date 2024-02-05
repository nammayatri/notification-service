package com.example.grpc;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.stub.StreamObserver;
public class Notification {
    private static NotificationGrpc.NotificationStub asyncStub;
    private ManagedChannel channel;

    Notification() {
        channel = ManagedChannelBuilder.forAddress("beta.beckn.uat.juspay.net", 50051)
                .intercept(new NotificationHeaderInterceptor())
                .keepAliveTime(15, TimeUnit.SECONDS)
                .build();
        asyncStub = NotificationGrpc.newStub(channel);
    }

    public static void streamPayload() throws Exception {
        NotificationResponseObserver notificationResponseObserver = new NotificationResponseObserver();
        StreamObserver<NotificationAck> notificationRequestObserver = asyncStub.streamPayload(notificationResponseObserver);
        notificationResponseObserver.startGRPCNotification(notificationRequestObserver);
    }

    // Method to close the gRPC channel
    public void closeChannel() {
        if (channel != null && !channel.isShutdown()) {
            try {
                channel.shutdown().awaitTermination(5, TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
    }

    // Override the finalize() method to ensure cleanup when the object is garbage collected
    @Override
    protected void finalize() throws Throwable {
        try {
            closeChannel();
        } finally {
            super.finalize();
        }
    }
}
