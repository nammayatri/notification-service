package com.example.grpc;

import static io.grpc.Metadata.ASCII_STRING_MARSHALLER;

import androidx.appcompat.app.AppCompatActivity;

import android.os.AsyncTask;
import android.os.Bundle;
import android.util.Log;

import java.io.PrintWriter;
import java.io.StringWriter;
import java.lang.ref.WeakReference;
import java.util.Date;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import io.grpc.CallOptions;
import io.grpc.Channel;
import io.grpc.ClientCall;
import io.grpc.ClientInterceptor;
import io.grpc.ForwardingClientCall;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Metadata;
import io.grpc.MethodDescriptor;
import io.grpc.stub.StreamObserver;

public class StreamActivity extends AppCompatActivity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_chat);
        new GrpcTask(new SendMessageRunnable(), this).execute();
    }

    private interface GrpcRunnable {
        /** Perform a grpcRunnable and return all the logs. */
        String run(ClientStreamGrpc.ClientStreamBlockingStub blockingStub, ClientStreamGrpc.ClientStreamStub asyncStub, ManagedChannel channel) throws Exception;
    }

    private static class GrpcTask extends AsyncTask<String, Void, String> {
        private final GrpcRunnable grpcRunnable;
        private ManagedChannel channel;
        private final WeakReference<StreamActivity> activityReference;

        GrpcTask(GrpcRunnable grpcRunnable, StreamActivity activity) {
            this.grpcRunnable = grpcRunnable;
            this.activityReference = new WeakReference<StreamActivity>(activity);
        }

        static class ClientStreamInterceptor implements ClientInterceptor {
            @Override
            public <ReqT, RespT> ClientCall<ReqT, RespT>
            interceptCall(
                    MethodDescriptor<ReqT, RespT> method, CallOptions callOptions, Channel next
            ) {
                return new
                        ForwardingClientCall.SimpleForwardingClientCall<ReqT, RespT>(next.newCall(method, callOptions)) {
                            @Override
                            public void start(ClientCall.Listener<RespT> responseListener, Metadata headers) {
                                headers.put(Metadata.Key.of("token", ASCII_STRING_MARSHALLER), "null");
                                headers.put(Metadata.Key.of("x-bundle-version", ASCII_STRING_MARSHALLER), "null");
                                headers.put(Metadata.Key.of("x-client-version", ASCII_STRING_MARSHALLER), "null");
                                super.start(responseListener, headers);
                            }
                        };
            }
        }

        String tryWithExponentialBackoff(int n, String logs) throws InterruptedException {
            int maxTimeMilliSecs = 64000, maxRetries = Integer.MAX_VALUE;
            if(n == maxRetries) {
                return logs;
            }
            try {
                ManagedChannel channel = ManagedChannelBuilder.forAddress("10.0.2.2", 50051).usePlaintext().intercept(new ClientStreamInterceptor())
//                .keepAliveTime(5, TimeUnit.SECONDS)
//                .keepAliveTimeout(20, TimeUnit.SECONDS)
                        .build();
                logs += ("\nSuccess!\n" + grpcRunnable.run(
                                ClientStreamGrpc.newBlockingStub(channel), ClientStreamGrpc.newStub(channel), channel));
            } catch (Exception e) {
                StringWriter sw = new StringWriter();
                PrintWriter pw = new PrintWriter(sw);
                e.printStackTrace(pw);
                pw.flush();
                logs += "\nFailed... :\n" + sw;
            }

            // min(((2^n)+random_number_milliseconds), maximum_backoff)
            TimeUnit.MILLISECONDS.sleep((long) Math.min(Math.pow(2,  n) + (Math.random() * 1001), maxTimeMilliSecs));
            tryWithExponentialBackoff(n + 1, logs);

            return logs;
        }

        @Override
        protected String doInBackground(String... strings) {
            try {
                return tryWithExponentialBackoff(1, "");
            } catch (InterruptedException e) {
                throw new RuntimeException(e);
            }
        }
    }

    private static class SendMessageRunnable implements GrpcRunnable {
        private Throwable failed;

        @Override
        public String run(ClientStreamGrpc.ClientStreamBlockingStub blockingStub, ClientStreamGrpc.ClientStreamStub asyncStub, ManagedChannel channel)
                throws Exception {
            return sendMessage(asyncStub, channel);
        }

        /**
         * Async client-streaming example. Sends {@code numPoints} randomly chosen points from {@code
         * features} with a variable delay in between. Prints the statistics when they are sent from the
         * server.
         */
        private String sendMessage(ClientStreamGrpc.ClientStreamStub asyncStub, ManagedChannel channel) throws InterruptedException, RuntimeException {
            final CountDownLatch finishLatch = new CountDownLatch(1);
            Log.i("GRPC", "sendMessage: ");
            StreamObserver<Empty> responseObserver =
                    new StreamObserver<Empty>() {
                        @Override
                        public void onNext(Empty response) {
                            Log.i("GRPC", "ResponseOnNext");
                        }

                        @Override
                        public void onError(Throwable t) {
                            Log.i("GRPC", "ResponseOnError : " + t);
                            channel.shutdown();
                            finishLatch.countDown();
                        }

                        @Override
                        public void onCompleted() {
                            Log.i("GRPC", "ResponseOnCompleted");
                            channel.shutdown();
                            finishLatch.countDown();
                        }
                    };

            StreamObserver<Message> requestObserver = asyncStub.sendMessage(responseObserver);

            try {
                // Send numPoints messages randomly selected from the messages list.
                // RPC completed or errored before we finished sending.
                // Sending further requests won't error, but they will just be thrown away.
                while(finishLatch.getCount() != 0) {
                    Message message = Message.newBuilder().setAccuracy(1).setTimestamp((new Date()).toString()).setLocation(Location.newBuilder().setLat(1.0).setLong(2.0).build()).build();
                    Messages messages = Messages.newBuilder().addMessages(message).build();
                    requestObserver.onNext(messages);

                    // Sleep for a bit before sending the next one.
                    Thread.sleep(3000);
                }
            } catch (RuntimeException e) {
                // Cancel RPC
                requestObserver.onError(e);
                throw e;
            }
            // Mark the end of requests
            requestObserver.onCompleted();

            // Receiving happens asynchronously
            if (!finishLatch.await(1, TimeUnit.MINUTES)) {
                throw new RuntimeException(
                        "Could not finish rpc within 1 minute, the server is likely down");
            }

            if (failed != null) {
                throw new RuntimeException(failed);
            }

            return "";
        }
    }
}