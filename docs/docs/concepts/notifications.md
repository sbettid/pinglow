---
sidebar_position: 2
---

# Notifications

Clearly, when a problem arise we would like to be notified. Pinglow supports notifications in the form of messages on Telegram.

It is so possible to define one or more `TelegramChannel` and use them associating to `Checks`.

Below we can find the definition for the `TelegramChannel`, with the `chatId` and the reference to a secret containing the property `botToken`. 

```yaml
apiVersion: pinglow.io/v1alpha1
kind: TelegramChannel
metadata:
  name: main-channel
spec:
  chatId: "-1234567890123"
  botTokenRef: "main-channel-token"
```

For more definition about the specific properties please see the [CRDs definition](https://github.com/sbettid/pinglow/blob/main/helm-charts/pinglow/templates/custom-rd.yaml).

Once a `TelegramChannel` is defined, we can associate it to a `Check` as follows: 

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: my-service-reachability
  namespace: pinglow
spec:
  scriptRef: check-service
  interval: 300
  secretRefs:
    - my-service-definition
  telegramChannelRefs: 
    - main-channel
```

Note that a check can send notifications also to more than one `TelegramChannel`.

## Muting/Unmuting notifications

Sometimes we would like to avoid notifications for a specific check for a certain period.

For this reason, Pinglow allows to mute/unmute notifications.

This is possible by setting the corresponding `muteNotifications` attribute (`true`/`false`) in the `Check` definition.
Furthermore, the `muteNotificationsUntil` attribute allow to specify until which date the notifications should be muted.
Note that not specifying it will mute notificatons for that check until they are explicitly removed.

Of course, muting notifications by modifiying the corresponding object in Kubernetes is not always the most comfortable way and, 
for this reason, this option is also available through the dedicated [RestAPI](/docs/restapi#operation/process_check_result).