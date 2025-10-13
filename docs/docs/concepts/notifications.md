---
sidebar_position: 2
---

# Notifications

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