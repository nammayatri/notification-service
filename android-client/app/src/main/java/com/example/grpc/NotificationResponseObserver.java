package com.example.grpc;

import android.util.Log;

import io.grpc.stub.StreamObserver;

public class NotificationResponseObserver implements StreamObserver<NotificationPayload> {

    private StreamObserver<NotificationAck> notificationRequestObserver;

    public void startGRPCNotification( StreamObserver<NotificationAck> notificationRequestObserver){
        this.notificationRequestObserver = notificationRequestObserver;
        Log.i("GRPC", "[Started]");
        this.notificationRequestObserver.onNext(NotificationAck.newBuilder().setId("").build());
    }

    @Override
    public void onNext(NotificationPayload value) {
        Log.i("GRPC", "[Message] : " + value.toString());
//        this.notificationRequestObserver.onNext(NotificationAck.newBuilder().setId(value.getId()).build());
    }

    @Override
    public void onError(Throwable t) {
        Log.e("GRPC", "[Error] : " + t.toString());
    }

    @Override
    public void onCompleted() {
        Log.e("GRPC", "[Completed]");
    }
}
