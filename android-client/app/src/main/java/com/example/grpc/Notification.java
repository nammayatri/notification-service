package com.example.grpc;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.stub.StreamObserver;
public class Notification {
    private static NotificationGrpc.NotificationStub asyncStub;

    Notification() {
        ManagedChannel channel = ManagedChannelBuilder.forAddress("grpc.beta.beckn.uat.juspay.net", 50051)
                .usePlaintext()
                .intercept(new NotificationHeaderInterceptor())
                .keepAliveWithoutCalls(true)
                .keepAliveTime(30, TimeUnit.SECONDS)
                .build();
        asyncStub = NotificationGrpc.newStub(channel);
    }

    public static void streamPayload() throws Exception {
        NotificationResponseObserver notificationResponseObserver = new NotificationResponseObserver();
        StreamObserver<NotificationAck> notificationRequestObserver = asyncStub.streamPayload(notificationResponseObserver);
        notificationResponseObserver.startGRPCNotification(notificationRequestObserver);
    }
}
