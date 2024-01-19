package com.example.grpc;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.stub.StreamObserver;
public class Notification {
    private static NotificationGrpc.NotificationStub asyncStub;

    Notification() {
        ManagedChannel channel = ManagedChannelBuilder.forAddress("a3d6a80d67fca4267b244647b5e05859-1229508546.ap-south-1.elb.amazonaws.com", 50051)
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
